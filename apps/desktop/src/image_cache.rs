//! A disk cache for every picture the app shows (store art, library art,
//! avatars, item icons, screenshots).
//!
//! It's an egui bytes loader registered in front of the normal HTTP one, so
//! all existing image code uses it without changes. A picture is downloaded
//! once, then read from disk on later starts (and offline). After a few days
//! it's revalidated in the background (ETag / Last-Modified) while the saved
//! copy keeps showing. Downloads run a few at a time, the folder is capped,
//! and the oldest pictures go first. Pictures that don't exist (a 404, e.g.
//! a game without library art) are remembered too, so the fallback art
//! shows at once on later starts instead of after another failed request.
//!
//! In memory, a picture is kept only as its GPU texture (no decoded or
//! downloaded copy), and only while it's on screen: a texture unused for
//! [`UNUSED_FOR`] is dropped and comes back from the disk cache when needed.

use egui::load::{Bytes, BytesLoadResult, BytesLoader, BytesPoll, LoadError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// Pictures older than this are checked again (in the background).
const FRESH_FOR: Duration = Duration::from_secs(3 * 24 * 60 * 60);
/// Cache size limit; trimmed back to 80% of it, oldest first.
const MAX_BYTES: u64 = 512 * 1024 * 1024;
/// Downloads at once (Steam's CDN drops connections when flooded).
const PARALLEL: usize = 8;
/// Textures not drawn (or prefetched) for this long are unloaded.
const MAX_TEXTURE_BYTES: usize = 96 * 1024 * 1024;
const UNUSED_FOR: Duration = Duration::from_secs(5);

#[derive(Clone)]
enum Entry {
    Pending,
    Ready(Bytes, Option<String>),
    Failed(String),
}

#[derive(Serialize, Deserialize, Default)]
struct Meta {
    mime: Option<String>,
    etag: Option<String>,
    last_modified: Option<String>,
}

pub struct ImageCache {
    dir: PathBuf,
    rt: tokio::runtime::Handle,
    http: reqwest::Client,
    slots: Arc<tokio::sync::Semaphore>,
    entries: Arc<Mutex<HashMap<String, Entry>>>,
}

/// Where pictures are kept: `<cache dir>/images`.
pub fn dir() -> PathBuf {
    directories::ProjectDirs::from("dev", "SteamInRust", "SteamInRust").map(|d| d.cache_dir().join("images")).unwrap_or_else(|| PathBuf::from("cache/images"))
}

/// Total size of the saved pictures.
pub fn size() -> u64 {
    std::fs::read_dir(dir()).map(|r| r.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0)
}

/// Delete every saved picture (Settings → Storage).
pub fn clear(ctx: &egui::Context) {
    let _ = std::fs::remove_dir_all(dir());
    ctx.forget_all_images();
}

impl ImageCache {
    pub fn install(ctx: &egui::Context, rt: tokio::runtime::Handle) {
        let dir = dir();
        let _ = std::fs::create_dir_all(&dir);
        static HTTP: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(build_http_client);
        static TRIMMED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        let http = HTTP.clone();
        // A second egui context shares the disk cache and the connection pool.
        TRIMMED.get_or_init(|| {
            let trim_dir = dir.clone();
            rt.spawn_blocking(move || trim(&trim_dir));
        });
        ctx.add_bytes_loader(Arc::new(Self { dir, rt, http, slots: Arc::new(tokio::sync::Semaphore::new(PARALLEL)), entries: Default::default() }));
        // Once a picture is a texture, drop its downloaded and decoded copies
        // (the disk cache has it), and unload textures that are off screen.
        ctx.options_mut(|o| o.reduce_texture_memory = true);
        ctx.add_texture_loader(Arc::new(UnloadUnused::default()));
        // Still pictures decoded at the size they're drawn at (sized_images.rs).
        crate::sized_images::SizedLoader::install(ctx);
    }

    fn key(uri: &str) -> String {
        const LUT: &[u8; 16] = b"0123456789abcdef";
        let digest = Sha256::digest(uri.as_bytes());
        let mut hex = [0u8; 32];
        for (i, &b) in digest[..16].iter().enumerate() {
            hex[i * 2] = LUT[(b >> 4) as usize];
            hex[i * 2 + 1] = LUT[(b & 0x0f) as usize];
        }
        // SAFETY: LUT contains only ASCII hex bytes.
        unsafe { String::from_utf8_unchecked(hex.to_vec()) }
    }
}

fn read_meta(path: &PathBuf) -> Meta {
    std::fs::read_to_string(path.with_extension("json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn age(path: &PathBuf) -> Option<Duration> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok())
}

impl BytesLoader for ImageCache {
    fn id(&self) -> &str {
        egui::generate_loader_id!(ImageCache)
    }

    fn load(&self, ctx: &egui::Context, uri: &str) -> BytesLoadResult {
        if !(uri.starts_with("https://") || uri.starts_with("http://")) {
            return Err(LoadError::NotSupported);
        }
        {
            let mut map = self.entries.lock().unwrap();
            if let Some(e) = map.get(uri) {
                return match e {
                    Entry::Pending => Ok(BytesPoll::Pending { size: None }),
                    Entry::Ready(bytes, mime) => Ok(BytesPoll::Ready { size: None, bytes: bytes.clone(), mime: mime.clone() }),
                    Entry::Failed(err) => Err(LoadError::Loading(err.clone())),
                };
            }
            if let Err(err) = validate_image_url(uri) {
                return Err(LoadError::Loading(err));
            }
            map.insert(uri.to_string(), Entry::Pending);
        }
        let (uri, ctx) = (uri.to_string(), ctx.clone());
        let path = self.dir.join(Self::key(&uri));
        let (http, slots, entries) = (self.http.clone(), self.slots.clone(), self.entries.clone());
        self.rt.spawn(async move {
            let done = |e: Entry| {
                entries.lock().unwrap().insert(uri.clone(), e);
                ctx.request_repaint();
            };
            // 1. Check disk cache off the async worker threads so bursts of cached
            // pictures never stall network / Steam CM tasks.
            let probe_path = path.clone();
            enum DiskHit {
                Fresh(Vec<u8>, Option<String>),
                Stale(Vec<u8>, Meta),
                KnownMissing(String),
                Miss,
            }
            let disk = tokio::task::spawn_blocking(move || {
                if let Ok(bytes) = std::fs::read(&probe_path) {
                    let meta = read_meta(&probe_path);
                    if age(&probe_path).is_some_and(|a| a < FRESH_FOR) {
                        DiskHit::Fresh(bytes, meta.mime)
                    } else {
                        DiskHit::Stale(bytes, meta)
                    }
                } else {
                    let missing = probe_path.with_extension("missing");
                    if age(&missing).is_some_and(|a| a < FRESH_FOR) {
                        let status = std::fs::read_to_string(&missing).unwrap_or_default();
                        DiskHit::KnownMissing(status)
                    } else {
                        DiskHit::Miss
                    }
                }
            })
            .await
            .unwrap_or(DiskHit::Miss);

            let (has_saved, meta) = match disk {
                DiskHit::Fresh(bytes, mime) => {
                    done(Entry::Ready(Bytes::from(bytes), mime));
                    return;
                }
                DiskHit::KnownMissing(status) => {
                    done(Entry::Failed(format!("HTTP {}", status.trim())));
                    return;
                }
                DiskHit::Stale(bytes, meta) => {
                    let mime = meta.mime.clone();
                    done(Entry::Ready(Bytes::from(bytes), mime));
                    (true, meta)
                }
                DiskHit::Miss => (false, Meta::default()),
            };
            // 2. Download, or revalidate an old copy.
            let _slot = slots.acquire().await;
            let mut req = http.get(&uri);
            if has_saved {
                if let Some(etag) = &meta.etag {
                    req = req.header(reqwest::header::IF_NONE_MATCH, etag);
                }
                if let Some(lm) = &meta.last_modified {
                    req = req.header(reqwest::header::IF_MODIFIED_SINCE, lm);
                }
            }
            match req.send().await {
                Ok(resp) if resp.remote_addr().is_some_and(|a| !is_public_ip(a.ip())) => {
                    if !has_saved {
                        done(Entry::Failed("blocked non-public image host".into()));
                    }
                }
                Ok(resp) if resp.status() == reqwest::StatusCode::NOT_MODIFIED => {
                    // Still current: mark it fresh again.
                    let touch_path = path.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Ok(f) = std::fs::File::options().append(true).open(&touch_path) {
                            let _ = f.set_modified(SystemTime::now());
                        }
                    })
                    .await;
                }
                Ok(resp) if resp.status().is_success() => {
                    let header = |name| resp.headers().get(name).and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok()).map(String::from);
                    let meta = Meta { mime: header(reqwest::header::CONTENT_TYPE), etag: header(reqwest::header::ETAG), last_modified: header(reqwest::header::LAST_MODIFIED) };
                    match read_image_bytes(resp).await {
                        Ok(bytes) => {
                            let shared = Bytes::from(bytes);
                            let mime = meta.mime.clone();
                            done(Entry::Ready(shared.clone(), mime));
                            let write_path = path.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                let tmp = write_path.with_extension("part");
                                if std::fs::write(&tmp, &shared[..]).is_ok() {
                                    let _ = std::fs::rename(&tmp, &write_path);
                                    let _ = std::fs::write(write_path.with_extension("json"), serde_json::to_string(&meta).unwrap_or_default());
                                }
                            })
                            .await;
                        }
                        Err(e) if !has_saved => done(Entry::Failed(e)),
                        Err(_) => {}
                    }
                }
                Ok(resp) if !has_saved => {
                    let status = resp.status();
                    // Remember pictures that don't exist (not server hiccups).
                    if matches!(status.as_u16(), 403 | 404 | 410) {
                        let missing = path.with_extension("missing");
                        let text = status.to_string();
                        let _ = tokio::task::spawn_blocking(move || {
                            let _ = std::fs::write(&missing, text);
                        })
                        .await;
                    }
                    done(Entry::Failed(format!("HTTP {status}")));
                }
                Err(e) if !has_saved => done(Entry::Failed(e.to_string())),
                // Offline or the server hiccuped: keep showing the saved copy.
                _ => {}
            }
        });
        Ok(BytesPoll::Pending { size: None })
    }

    fn forget(&self, uri: &str) {
        self.entries.lock().unwrap().remove(uri);
    }

    fn forget_all(&self) {
        self.entries.lock().unwrap().clear();
    }

    fn byte_size(&self) -> usize {
        self.entries.lock().unwrap().values().map(|e| if let Entry::Ready(b, _) = e { b.len() } else { 0 }).sum()
    }
}

/// The standard texture cache, plus unloading: a texture that hasn't been
/// drawn for [`UNUSED_FOR`] is dropped (scrolled away, another page open).
#[derive(Default)]
struct UnloadUnused {
    inner: egui::load::DefaultTextureLoader,
    /// When each loaded texture was last asked for.
    used: Mutex<HashMap<String, std::time::Instant>>,
    last_sweep: Mutex<Option<std::time::Instant>>,
}

impl egui::load::TextureLoader for UnloadUnused {
    fn id(&self) -> &str {
        egui::generate_loader_id!(UnloadUnused)
    }

    fn load(&self, ctx: &egui::Context, uri: &str, texture_options: egui::TextureOptions, size_hint: egui::SizeHint) -> egui::load::TextureLoadResult {
        let result = self.inner.load(ctx, uri, texture_options, size_hint);
        if matches!(result, Ok(egui::load::TexturePoll::Ready { .. })) {
            let now = std::time::Instant::now();
            let mut used = self.used.lock().unwrap();
            match used.get_mut(uri) {
                Some(t) => *t = now,
                None => {
                    used.insert(uri.to_owned(), now);
                }
            }
        }
        result
    }

    fn forget(&self, uri: &str) {
        self.used.lock().unwrap().remove(uri);
        self.inner.forget(uri);
    }

    fn forget_all(&self) {
        self.used.lock().unwrap().clear();
        self.inner.forget_all();
    }

    fn end_pass(&self, pass_index: u64) {
        self.inner.end_pass(pass_index);
        let now = std::time::Instant::now();
        {
            let mut last = self.last_sweep.lock().unwrap();
            if last.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1)) {
                return;
            }
            *last = Some(now);
        }
        let stale: Vec<String> = {
            let mut used = self.used.lock().unwrap();
            let stale: Vec<String> = used.iter().filter(|(_, t)| now.duration_since(**t) > UNUSED_FOR).map(|(uri, _)| uri.clone()).collect();
            for uri in &stale {
                used.remove(uri);
            }
            stale
        };
        for uri in stale {
            self.inner.forget(&uri);
        }
        if self.inner.byte_size() > MAX_TEXTURE_BYTES {
            let mut used = self.used.lock().unwrap();
            // Keep textures used in the current pass. Evict older ones first.
            let mut candidates: Vec<_> = used.iter()
                .filter(|(_, t)| now.duration_since(**t) > Duration::from_millis(200))
                .map(|(uri, t)| (*t, uri.clone()))
                .collect();
            candidates.sort_unstable_by_key(|(t, _)| *t);
            for (_, uri) in candidates {
                if self.inner.byte_size() <= MAX_TEXTURE_BYTES {
                    break;
                }
                used.remove(&uri);
                self.inner.forget(&uri);
            }
        }
    }

    fn byte_size(&self) -> usize {
        self.inner.byte_size()
    }
}

/// Delete the oldest pictures until the folder is under 80% of the limit.
fn trim(dir: &PathBuf) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = read
        .flatten()
        .filter(|e| e.path().extension().is_none())
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= MAX_BYTES {
        return;
    }
    files.sort_by_key(|f| f.0);
    for (_, len, path) in files {
        if total <= MAX_BYTES / 10 * 8 {
            break;
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("json"));
        total = total.saturating_sub(len);
    }
}

const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;

fn is_public_ipv4(ip: std::net::Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || a == 0
        || (a == 100 && (b & 0b1100_0000) == 64)
        || (a == 198 && (b & 0b1111_1110) == 18)
        || a >= 240)
}

fn is_public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => is_public_ipv4(v4),
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
                return false;
            }
            if let Some(v4) = v6.to_ipv4_mapped().or_else(|| v6.to_ipv4()) {
                return is_public_ipv4(v4);
            }
            let segs = v6.segments();
            !((segs[0] & 0xfe00) == 0xfc00
                || (segs[0] & 0xffc0) == 0xfe80
                || (segs[0] == 0x2001 && segs[1] == 0x0db8))
        }
    }
}

fn validate_parsed_image_url(url: &reqwest::Url) -> Result<(), String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("unsupported image scheme".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("image URLs cannot carry credentials".into());
    }
    let Some(host) = url.host_str() else {
        return Err("image URL missing host".into());
    };
    let clean = host.trim_end_matches('.');
    let unbracketed = clean.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(clean);
    if let Ok(ip) = unbracketed.parse::<std::net::IpAddr>() {
        if !is_public_ip(ip) {
            return Err("blocked non-public IP address".into());
        }
        return Ok(());
    }
    let lower = unbracketed.to_ascii_lowercase();
    if lower.is_empty()
        || lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
    {
        return Err("blocked local image hostname".into());
    }
    Ok(())
}

fn resolve_public_addrs(host: &str) -> Result<Vec<std::net::SocketAddr>, String> {
    use std::net::ToSocketAddrs;
    let mut out = Vec::new();
    for addr in (host, 0).to_socket_addrs().map_err(|e| e.to_string())? {
        if !is_public_ip(addr.ip()) {
            return Err("blocked non-public DNS resolution".into());
        }
        out.push(addr);
    }
    if out.is_empty() {
        return Err("DNS returned no addresses".into());
    }
    Ok(out)
}

struct PublicDnsResolver;

impl reqwest::dns::Resolve for PublicDnsResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs = tokio::task::spawn_blocking(move || resolve_public_addrs(&host))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
                .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::PermissionDenied, e)) as Box<dyn std::error::Error + Send + Sync>)?;
            let iter: reqwest::dns::Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

fn is_trusted_steam_cdn_host(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    matches!(
        h.as_str(),
        "steamusercontent-a.akamaihd.net"
            | "steamuserimages-a.akamaihd.net"
            | "steamcdn-a.akamaihd.net"
            | "media.steampowered.com"
            | "avatars.akamai.steamstatic.com"
            | "avatars.fastly.steamstatic.com"
            | "avatars.steamstatic.com"
    ) || h.ends_with(".steamusercontent.com")
        || h.ends_with(".steamstatic.com")
}

fn build_http_client() -> reqwest::Client {
    let redirect = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            return attempt.error("too many redirects");
        }
        if let Err(err) = validate_parsed_image_url(attempt.url()) {
            return attempt.error(err);
        }
        if let Some(origin) = attempt.previous().first() {
            if origin.scheme() == "https" && attempt.url().scheme() != "https" {
                return attempt.error("refusing HTTPS-to-HTTP image redirect");
            }
            if origin.host_str().is_some_and(is_trusted_steam_cdn_host)
                && !attempt.url().host_str().is_some_and(is_trusted_steam_cdn_host)
            {
                return attempt.error("refusing redirect off Steam CDN");
            }
        }
        attempt.follow()
    });
    reqwest::Client::builder()
        .user_agent(concat!("Aurora/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .redirect(redirect)
        .dns_resolver(Arc::new(PublicDnsResolver))
        .build()
        .unwrap_or_default()
}

fn validate_image_url(uri: &str) -> Result<reqwest::Url, String> {
    if uri.bytes().any(|b| b < 0x20 || b == 0x7f) || uri.contains('\\') {
        return Err("invalid character in image URL".into());
    }
    let url = reqwest::Url::parse(uri).map_err(|e| e.to_string())?;
    validate_parsed_image_url(&url)?;
    Ok(url)
}

async fn read_image_bytes(mut resp: reqwest::Response) -> Result<Vec<u8>, String> {
    if resp.content_length().is_some_and(|len| len > MAX_IMAGE_BYTES as u64) {
        return Err("image exceeds 16 MiB limit".into());
    }
    let cap = resp.content_length().map(|n| (n as usize).min(MAX_IMAGE_BYTES)).unwrap_or(0);
    let mut buf = Vec::with_capacity(cap);
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if buf.len().saturating_add(chunk.len()) > MAX_IMAGE_BYTES {
            return Err("image exceeds 16 MiB limit".into());
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn rejects_ssrf_private_loopback_and_link_local_urls() {
        for bad in [
            "http://127.0.0.1/a.png",
            "https://127.0.1.1:8080/a.png",
            "http://[::1]/a.png",
            "http://[::ffff:127.0.0.1]/a.png",
            "http://localhost/a.png",
            "https://sub.localhost/a.png",
            "http://169.254.169.254/latest/meta-data/",
            "http://[fe80::1]/a.png",
            "http://10.0.0.1/a.png",
            "http://172.16.0.1/a.png",
            "http://192.168.1.1/a.png",
            "http://[fc00::1]/a.png",
            "http://[fd12:3456::1]/a.png",
            "http://0.0.0.0/a.png",
        ] {
            assert!(validate_image_url(bad).is_err(), "must reject SSRF url {bad}");
        }
        assert!(validate_image_url("https://shared.fastly.steamstatic.com/store_item_assets/steam/apps/440/header.jpg").is_ok());
    }

    #[tokio::test]
    async fn blocks_redirect_ssrf_and_caps_streaming_body_at_16_mib() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let srv = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut req = [0u8; 1024];
            let _ = sock.read(&mut req);
            let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 17825792\r\nConnection: close\r\n\r\n");
            let block = vec![0u8; 1024 * 1024];
            for _ in 0..17 {
                if sock.write_all(&block).is_err() {
                    break;
                }
            }
        });
        let raw_client = reqwest::Client::new();
        let resp = raw_client.get(format!("http://{addr}/bomb.png")).send().await.unwrap();
        assert!(read_image_bytes(resp).await.is_err(), "image > 16 MiB must be rejected");
        tokio::task::spawn_blocking(move || srv.join().unwrap()).await.unwrap();

        // Also test chunked body without Content-Length exceeding 16 MiB.
        let chunked = TcpListener::bind("127.0.0.1:0").unwrap();
        let caddr = chunked.local_addr().unwrap();
        let csrv = std::thread::spawn(move || {
            let (mut sock, _) = chunked.accept().unwrap();
            let mut req = [0u8; 1024];
            let _ = sock.read(&mut req);
            let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
            let chunk_hdr = format!("{:x}\r\n", 1024 * 1024);
            let block = vec![b'a'; 1024 * 1024];
            for _ in 0..17 {
                if sock.write_all(chunk_hdr.as_bytes()).is_err()
                    || sock.write_all(&block).is_err()
                    || sock.write_all(b"\r\n").is_err()
                {
                    break;
                }
            }
            let _ = sock.write_all(b"0\r\n\r\n");
        });
        let resp = raw_client.get(format!("http://{caddr}/chunked.png")).send().await.unwrap();
        assert!(read_image_bytes(resp).await.is_err(), "chunked image > 16 MiB must be rejected");
        drop(raw_client);
        tokio::task::spawn_blocking(move || csrv.join().unwrap()).await.unwrap();

        // ImageCache HTTP client redirect policy must reject loopback/private redirects.
        let redir = TcpListener::bind("127.0.0.1:0").unwrap();
        let raddr = redir.local_addr().unwrap();
        let rsrv = std::thread::spawn(move || {
            let (mut sock, _) = redir.accept().unwrap();
            let mut req = [0u8; 1024];
            let _ = sock.read(&mut req);
            let _ = sock.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        });
        let cache_http = build_http_client();
        let err = cache_http.get(format!("http://{raddr}/redir")).send().await.unwrap_err();
        assert!(err.is_redirect(), "redirect to link-local IP must be blocked, got {err}");
        tokio::task::spawn_blocking(move || rsrv.join().unwrap()).await.unwrap();

        assert!(validate_image_url("http://2130706433/a.png").is_err());
        assert!(validate_image_url("http://0177.0.0.1/a.png").is_err());
        assert!(resolve_public_addrs("127.0.0.1").is_err());
        assert!(resolve_public_addrs("169.254.169.254").is_err());
    }
}
