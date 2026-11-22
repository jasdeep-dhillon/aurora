//! Download regions: every Steam content-server region, with the download
//! speed measured from this PC, sorted fastest first.
//!
//! Speed, not ping: the regions with the shortest connection times (and the
//! partner CDNs) each download the same real game chunk, one after another,
//! and are ranked by how fast that went. The chunk comes from a manifest in
//! Steam's `depotcache`; without one, the list falls back to ping order.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Steam region ids to look through. Each answers with servers in its own
/// region (or the nearest one), so together they cover every region.
const CELLS: std::ops::RangeInclusive<u32> = 1..=230;

/// Airport-style codes in server names -> city.
fn city(code: &str) -> String {
    let name = match code {
        "ams" => "Amsterdam", "atl" => "Atlanta", "bom" => "Mumbai", "can" => "Guangzhou", "cdg" | "par" => "Paris",
        "chi" | "ord" => "Chicago", "dfw" => "Dallas", "dxb" => "Dubai", "eze" => "Buenos Aires", "fra" => "Frankfurt",
        "gru" => "São Paulo", "hel" => "Helsinki", "hkg" => "Hong Kong", "iad" => "Washington, D.C.", "icn" | "sel" => "Seoul",
        "jnb" => "Johannesburg", "lax" => "Los Angeles", "lhr" | "lon" => "London", "lim" => "Lima", "mad" => "Madrid",
        "man" => "Manchester", "mel" => "Melbourne", "mia" => "Miami", "mil" | "mxp" => "Milan", "mow" | "svo" | "dme" => "Moscow",
        "nrt" | "tyo" | "hnd" => "Tokyo", "nyc" | "jfk" | "ewr" => "New York", "osa" | "kix" => "Osaka", "per" => "Perth",
        "prg" => "Prague", "scl" => "Santiago", "sea" => "Seattle", "sgp" | "sin" => "Singapore", "sto" | "arn" => "Stockholm",
        "syd" => "Sydney", "tpe" => "Taipei", "vie" => "Vienna", "waw" => "Warsaw", "led" => "Saint Petersburg", "ala" => "Almaty",
        "ist" => "Istanbul", "bkk" => "Bangkok", "kul" => "Kuala Lumpur", "mnl" => "Manila", "sjc" => "San Jose", "den" => "Denver",
        "yyz" => "Toronto", "yvr" => "Vancouver", "bog" => "Bogotá", "mex" => "Mexico City", "tlv" => "Tel Aviv", "cph" => "Copenhagen",
        "bru" => "Brussels", "zrh" => "Zurich", "dub" => "Dublin", "osl" => "Oslo", "lis" => "Lisbon", "bud" => "Budapest",
        "buh" | "otp" => "Bucharest", "kiv" => "Chișinău", "kbp" | "iev" => "Kyiv", "tas" => "Tashkent", "nqz" | "tse" => "Astana",
        "akl" => "Auckland", "bne" => "Brisbane", "hyd" => "Hyderabad", "maa" => "Chennai", "del" => "Delhi", "cgk" | "jkt" => "Jakarta",
        "sha" | "pvg" => "Shanghai", "pek" | "bjs" => "Beijing", "ctu" => "Chengdu", "szx" => "Shenzhen", "wuh" => "Wuhan",
        _ => return code.to_uppercase(),
    };
    name.to_string()
}

/// Steam's partner CDNs, by host name.
fn cdn_name(host: &str) -> String {
    let partner = ["akamai", "fastly", "google", "alibaba", "edgenext", "cloudflare", "level3"].into_iter().find(|p| host.contains(p)).unwrap_or("CDN");
    let mut name = partner.to_string();
    if let Some(c) = name.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    format!("{name} (CDN)")
}

/// "cache5-fra2.steamcontent.com" -> "fra".
fn code(host: &str) -> Option<String> {
    let first = host.split('.').next()?;
    let part = first.split('-').nth(1)?;
    let letters: String = part.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    (letters.len() >= 3).then_some(letters.to_lowercase())
}

/// How many of the lowest-ping regions get a speed test (CDNs always do).
const SPEED_TESTS: usize = 10;

/// A chunk to test with: (depot, sha, compressed size), between 256 KB and
/// 4 MB, the biggest found in the first manifests of `<steam_root>/depotcache`.
fn test_chunk(steam_root: &std::path::Path) -> Option<(u32, Vec<u8>, u32)> {
    let mut files: Vec<_> = std::fs::read_dir(steam_root.join("depotcache")).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "manifest")).collect();
    files.sort();
    let mut best: Option<(u32, Vec<u8>, u32)> = None;
    for path in files.iter().take(40) {
        let Some(depot) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.split('_').next()).and_then(|d| d.parse().ok()) else { continue };
        let Some(m) = std::fs::read(path).ok().and_then(|b| depot::Manifest::parse(&b).ok()) else { continue };
        for c in m.files.iter().flat_map(|f| &f.chunks) {
            if (256 << 10..=4 << 20).contains(&c.compressed) && best.as_ref().is_none_or(|b| c.compressed > b.2) {
                best = Some((depot, c.sha.clone(), c.compressed));
            }
        }
        if best.as_ref().is_some_and(|b| b.2 >= 1 << 20) {
            break;
        }
    }
    best
}

/// KB/s for downloading the test chunk from `host`, best of two on the same
/// connection (the first one also pays for TLS and TCP slow start); 0 if
/// the server didn't deliver it.
async fn speed(http: &reqwest::Client, host: &str, chunk: &(u32, Vec<u8>, u32)) -> u32 {
    let hex: String = chunk.1.iter().map(|b| format!("{b:02x}")).collect();
    let url = format!("https://{host}/depot/{}/chunk/{hex}", chunk.0);
    let mut best = 0;
    for _ in 0..2 {
        let start = Instant::now();
        let got = tokio::time::timeout(Duration::from_secs(8), async { http.get(&url).send().await?.error_for_status()?.bytes().await }).await;
        let Ok(Ok(body)) = got else { break };
        if (body.len() as u32) < chunk.2 / 2 {
            break;
        }
        best = best.max((body.len() as f64 / 1024.0 / start.elapsed().as_secs_f64().max(0.001)) as u32);
    }
    best
}

/// Look up every region, time a connection to one server in each, then
/// measure download speed from the closest ones.
pub async fn probe(steam_root: Option<std::path::PathBuf>) -> Vec<model::DownloadRegion> {
    static HTTP: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder().user_agent("Valve/Steam HTTP Client 1.0").timeout(Duration::from_secs(10)).build().unwrap_or_default()
    });
    let http = HTTP.clone();
    // 1) Which regions exist.
    let mut found: BTreeMap<u32, (String, u32)> = BTreeMap::new(); // cell -> (host, load)
    let mut cdns: BTreeMap<String, u32> = BTreeMap::new(); // partner CDN host -> load
    let cells: Vec<u32> = CELLS.collect();
    for chunk in cells.chunks(24) {
        let replies = futures_util::future::join_all(chunk.iter().map(|cell| {
            let http = http.clone();
            async move {
                let v: serde_json::Value = http
                    .get("https://api.steampowered.com/IContentServerDirectoryService/GetServersForSteamPipe/v1/")
                    .query(&[("cell_id", cell.to_string()), ("max_servers", "3".into())])
                    .send()
                    .await
                    .ok()?
                    .json()
                    .await
                    .ok()?;
                Some(v)
            }
        }))
        .await;
        for v in replies.into_iter().flatten() {
            for s in v.pointer("/response/servers").and_then(|s| s.as_array()).into_iter().flatten() {
                let cell = s.get("cell_id").and_then(|c| c.as_u64()).unwrap_or(0) as u32;
                let host = s.get("host").and_then(|h| h.as_str()).unwrap_or_default().to_string();
                let load = s.get("load").and_then(|l| l.as_u64()).unwrap_or(0) as u32;
                match s.get("type").and_then(|t| t.as_str()) {
                    Some("SteamCache") if cell != 0 && code(&host).is_some() => {
                        found.entry(cell).or_insert((host, load));
                    }
                    Some("CDN") if !host.is_empty() => {
                        cdns.entry(host).or_insert(load);
                    }
                    _ => {}
                }
            }
        }
    }
    // 2) Time a TCP connection to each (best of three).
    let mut targets: Vec<(u32, String, u32, Option<String>)> = found.into_iter().map(|(cell, (host, load))| (cell, host, load, None)).collect();
    targets.extend(cdns.into_iter().map(|(host, load)| (0, host.clone(), load, Some(cdn_name(&host)))));
    let timed = futures_util::future::join_all(targets.into_iter().map(|(cell, host, load, cdn)| async move {
        let mut best: Option<u32> = None;
        for _ in 0..3 {
            let start = Instant::now();
            let ok = tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect((host.as_str(), 443))).await;
            if matches!(ok, Ok(Ok(_))) {
                let ms = start.elapsed().as_millis() as u32;
                best = Some(best.map_or(ms, |b| b.min(ms)));
            }
        }
        let code = code(&host).unwrap_or_default();
        let name = cdn.unwrap_or_else(|| city(&code));
        model::DownloadRegion { cell_id: cell, name, code, host, load, ping_ms: best, speed_kbps: None }
    }))
    .await;
    let mut out: Vec<model::DownloadRegion> = timed;
    out.sort_by_key(|r| (r.ping_ms.unwrap_or(u32::MAX), r.load));
    // 3) Download speed, one server at a time so they don't share the line.
    let chunk = match steam_root {
        Some(root) => tokio::task::spawn_blocking(move || test_chunk(&root)).await.ok().flatten(),
        None => None,
    };
    if let Some(chunk) = &chunk {
        let mut tested = 0;
        for r in out.iter_mut().filter(|r| r.ping_ms.is_some()) {
            if r.cell_id != 0 && tested >= SPEED_TESTS {
                continue;
            }
            tested += usize::from(r.cell_id != 0);
            r.speed_kbps = Some(speed(&http, &r.host, chunk).await);
        }
        tracing::info!("download regions: speed-tested with a {} KB chunk of depot {}", chunk.2 / 1024, chunk.0);
        // Measured ones first, fastest first; then the untested ones in ping
        // order; servers that failed the test last.
        out.sort_by_key(|r| (r.speed_kbps == Some(0), r.speed_kbps.is_none(), std::cmp::Reverse(r.speed_kbps.unwrap_or(0))));
    }
    // One entry per city: keep the fastest.
    let mut seen = std::collections::HashSet::new();
    out.retain(|r| seen.insert(r.name.clone()));
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_server_codes() {
        assert_eq!(super::code("cache5-fra2.steamcontent.com").as_deref(), Some("fra"));
        assert_eq!(super::city("ams"), "Amsterdam");
        assert_eq!(super::code("steampipe.akamaized.net"), None);
    }
}
