//! Live HLS stream player (for Steam Broadcasts) with audio, pause/unpause,
//! volume/mute, and a 30-second in-memory DVR rewind buffer.
//!
//! Unlike VOD trailers (fixed `#EXT-X-ENDLIST` playlists), Steam Broadcasts use
//! sliding live HLS playlists with non-zero live PTS offsets. On Linux we let
//! `ffmpeg` handle live playlist reloading, reconnection, and A/V pacing
//! natively, streaming 960×540 RGB24 frames to egui and 48 kHz stereo f32 PCM
//! to `pulse::Output`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub const LIVE_W: usize = 960;
pub const LIVE_H: usize = 540;
const DVR_W: usize = LIVE_W / 2;
const DVR_H: usize = LIVE_H / 2;
const LIVE_FPS: u32 = 30;
const DVR_FPS: u32 = 10;
const DVR_STRIDE: u32 = LIVE_FPS / DVR_FPS; // push 1 of every 3 live frames
const DVR_MAX_CHUNKS: usize = 300; // 30 seconds at 10 Hz (0.1s per chunk)
const AUDIO_RATE: u32 = 48_000;
const AUDIO_CHANNELS: usize = 2;
const AUDIO_CHUNK_SAMPLES: usize = (AUDIO_RATE as usize / DVR_FPS as usize) * AUDIO_CHANNELS; // 9600 f32 = 0.1s

/// Validate a live HLS URL before handing it to `ffmpeg`:
/// requires `https://`, no control characters, and rejects loopback/private/link-local hosts.
pub fn validate_live_hls_url(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() || s.starts_with('-') || s.chars().any(|c| c.is_control()) {
        return Err("Unsupported broadcast stream URL.".into());
    }
    let parsed = reqwest::Url::parse(s).map_err(|_| "Invalid broadcast stream URL.".to_string())?;
    if parsed.scheme() != "https" {
        return Err("Broadcast stream must use HTTPS.".into());
    }
    let Some(host) = parsed.host_str().filter(|h| !h.is_empty()) else {
        return Err("Broadcast stream URL has no host.".into());
    };
    let host_unbracketed = host.trim_start_matches('[').trim_end_matches(']');
    let is_private_host = if let Ok(ip) = host_unbracketed.parse::<std::net::IpAddr>() {
        match ip {
            std::net::IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_broadcast()
                    || v4.is_multicast()
                    || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
            }
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00
                    || (v6.segments()[0] & 0xffc0) == 0xfe80
                    || v6.to_ipv4_mapped().is_some_and(|v4| {
                        v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
                    })
            }
        }
    } else {
        let dl = host_unbracketed.to_ascii_lowercase();
        dl == "localhost" || dl.ends_with(".localhost") || dl.ends_with(".local") || dl.ends_with(".internal")
    };
    if is_private_host {
        return Err("Broadcast stream URL points to a disallowed local or private address.".into());
    }
    Ok(parsed.to_string())
}

struct DvrState {
    /// Latest full-res 960×540 frame, expanded on the decoder thread.
    live_frame: Option<Arc<egui::ColorImage>>,
    /// 480×270 RGB24 frames at 10 Hz (newest at back), up to 30 seconds.
    frames: VecDeque<Vec<u8>>,
    /// 0.1s stereo 48 kHz f32 PCM chunks (newest at back), up to 30 seconds.
    audio: VecDeque<Vec<f32>>,
    /// Seconds behind live (`0.0` = live head).
    behind: f32,
    /// Paused playback: video stays pinned and audio is silent.
    paused: bool,
    /// Monotonic counter bumped whenever the displayed frame changes.
    revision: u64,
}

impl Default for DvrState {
    fn default() -> Self {
        Self {
            live_frame: None,
            frames: VecDeque::new(),
            audio: VecDeque::new(),
            behind: 0.0,
            paused: false,
            revision: 0,
        }
    }
}

impl DvrState {
    fn buffered_secs(&self) -> f32 {
        (self.frames.len().saturating_sub(1)) as f32 / DVR_FPS as f32
    }

    fn is_live(&self) -> bool {
        self.behind <= 0.15
    }

    fn clamp_behind(&mut self) {
        let max = self.buffered_secs();
        self.behind = self.behind.clamp(0.0, max);
    }

    fn push_video(&mut self, image: Arc<egui::ColorImage>, rgb: &[u8], push_dvr: bool) -> Option<Arc<egui::ColorImage>> {
        if push_dvr {
            let mut small = if self.frames.len() >= DVR_MAX_CHUNKS {
                self.frames.pop_front().unwrap()
            } else {
                vec![0; DVR_W * DVR_H * 3]
            };
            downsample_2x_rgb(rgb, LIVE_W, LIVE_H, &mut small);
            self.frames.push_back(small);
            if self.paused {
                // Keep the frozen frame pinned as new live frames arrive.
                self.behind = (self.behind + 1.0 / DVR_FPS as f32).min(self.buffered_secs());
            } else {
                self.clamp_behind();
            }
        }
        let old = self.live_frame.replace(image);
        if !self.paused && (self.is_live() || push_dvr) {
            self.revision = self.revision.wrapping_add(1);
        }
        old
    }

    fn push_audio_and_pick(&mut self, live_chunk: Vec<f32>) -> Option<Vec<f32>> {
        if self.audio.len() >= DVR_MAX_CHUNKS {
            self.audio.pop_front();
        }
        self.audio.push_back(live_chunk);
        if self.paused {
            return None;
        }
        if self.is_live() {
            return self.audio.back().cloned();
        }
        let back_idx = (self.behind * DVR_FPS as f32).round() as usize;
        let idx = self.audio.len().saturating_sub(1).saturating_sub(back_idx);
        self.audio.get(idx).cloned()
    }

    fn current_frame(&self) -> Option<Arc<egui::ColorImage>> {
        if self.is_live() && !self.paused {
            return self.live_frame.clone();
        }
        let back_idx = (self.behind * DVR_FPS as f32).round() as usize;
        let idx = self.frames.len().saturating_sub(1).saturating_sub(back_idx);
        self.frames.get(idx)
            .map(|rgb| Arc::new(egui::ColorImage::from_rgb([DVR_W, DVR_H], rgb)))
            .or_else(|| self.live_frame.clone())
    }

}

/// Fast 2×2 nearest downsample of an RGB24 frame (`w×h` -> `(w/2)×(h/2)`).
fn downsample_2x_rgb(src: &[u8], w: usize, h: usize, dst: &mut [u8]) {
    let dw = w / 2;
    let dh = h / 2;
    for y in 0..dh {
        let sy = y * 2;
        let row_src = sy * w * 3;
        let row_dst = y * dw * 3;
        for x in 0..dw {
            let si = row_src + x * 6;
            let di = row_dst + x * 3;
            dst[di] = src[si];
            dst[di + 1] = src[si + 1];
            dst[di + 2] = src[si + 2];
        }
    }
}

pub struct LivePlayer {
    dvr: Arc<Mutex<DvrState>>,
    uploaded_rev: u64,
    texture: Option<egui::TextureHandle>,
    muted: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    has_frame: Arc<AtomicBool>,
    #[cfg(not(windows))]
    procs: Arc<Mutex<Vec<std::process::Child>>>,
}

impl LivePlayer {
    pub fn start(ctx: &egui::Context, url: String) -> Self {
        Self::start_with_auth(ctx, url, String::new())
    }

    pub fn start_with_auth(ctx: &egui::Context, url: String, auth_params: String) -> Self {
        let dvr = Arc::new(Mutex::new(DvrState::default()));
        let muted = Arc::new(AtomicBool::new(false));
        let volume = Arc::new(AtomicU32::new(0.8_f32.to_bits()));
        let stop = Arc::new(AtomicBool::new(false));
        let error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let has_frame = Arc::new(AtomicBool::new(false));

        #[cfg(not(windows))]
        let procs: Arc<Mutex<Vec<std::process::Child>>> = Arc::new(Mutex::new(Vec::new()));

        let validated_url = match validate_live_hls_url(&url) {
            Ok(u) => Some(u),
            Err(e) => {
                *error.lock().unwrap() = Some(e);
                None
            }
        };

        #[cfg(not(windows))]
        if let Some(url) = validated_url {
            use std::io::{BufRead, Read};
            use std::os::fd::OwnedFd;
            use std::os::unix::net::UnixStream;
            use std::process::{Command, Stdio};

            // Steam's Akamai HLS broadcasts require `cdn_auth_url_parameters`
            // on every segment/init-section request, which FFmpeg's HLS demuxer
            // does not propagate from master.m3u8. Rewrite only the tiny .m3u8
            // playlists on loopback so segments still stream directly from CDN.
            let uses_local_proxy = !auth_params.is_empty();
            let url = if uses_local_proxy {
                start_hls_auth_proxy(url, auth_params, stop.clone()).unwrap_or_else(|e| {
                    tracing::warn!("broadcast auth proxy unavailable: {e}");
                    String::new()
                })
            } else {
                url
            };
            let whitelist = if uses_local_proxy { "http,https,tls,tcp,crypto" } else { "https,tls,tcp,crypto" };

            let bin = std::env::var("STEAMINRUST_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());

            // FFmpeg writes PCM to its full-duplex stdin socket. -nostdin
            // disables interactive reads, leaving stderr for diagnostics.
            if let Ok((mut audio_input, audio_output)) = UnixStream::pair() {
                let bin_v = bin.clone();
                let url_v = url.clone();
                let dvr_v = dvr.clone();
                let stop_v = stop.clone();
                let err_v = error.clone();
                let has_v = has_frame.clone();
                let procs_v = procs.clone();
                let ctx_v = ctx.clone();
                let _ = std::thread::Builder::new().name("live-video".into()).spawn(move || {
                    let frame_bytes = LIVE_W * LIVE_H * 3;
                    let vf = format!(
                        "scale={LIVE_W}:{LIVE_H}:force_original_aspect_ratio=decrease,pad={LIVE_W}:{LIVE_H}:(ow-iw)/2:(oh-ih)/2"
                    );
                    let child = Command::new(&bin_v)
                        .args([
                            "-hide_banner",
                            "-loglevel",
                            "error",
                            "-nostdin",
                            "-protocol_whitelist",
                            whitelist,
                            "-extension_picky",
                            "0",
                            "-allowed_extensions",
                            "ALL",
                            "-allowed_segment_extensions",
                            "ALL",
                            "-reconnect",
                            "1",
                            "-reconnect_streamed",
                            "1",
                            "-reconnect_delay_max",
                            "2",
                            "-i",
                            &url_v,
                            "-map",
                            "0:v:0",
                            "-map",
                            "0:a:0?",
                            "-c:v",
                            "rawvideo",
                            "-vf",
                            &vf,
                            "-r",
                            "30",
                            "-pix_fmt",
                            "rgb24",
                            "-c:a",
                            "pcm_f32le",
                            "-ar",
                            "48000",
                            "-ac",
                            "2",
                            "-f",
                            "tee",
                            "[select=v:f=rawvideo]pipe:1|[select=a:onfail=ignore:f=f32le]pipe:0",
                        ])
                        .stdin(Stdio::from(OwnedFd::from(audio_output)))
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn();
                    let mut child = match child {
                        Ok(c) => c,
                        Err(e) => {
                            *err_v.lock().unwrap() = Some(format!("couldn't start ffmpeg: {e}"));
                            ctx_v.request_repaint();
                            return;
                        }
                    };
                    let Some(mut stdout) = child.stdout.take() else {
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    };
                    let diagnostic = Arc::new(Mutex::new(None));
                    if let Some(stderr) = child.stderr.take() {
                        let diagnostic = diagnostic.clone();
                        std::thread::spawn(move || {
                            for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
                                let mut first = diagnostic.lock().unwrap();
                                if first.is_none() {
                                    *first = Some(line);
                                }
                            }
                        });
                    }
                    {
                        let mut children = procs_v.lock().unwrap();
                        if stop_v.load(Ordering::Acquire) {
                            let _ = child.kill();
                            let _ = child.wait();
                            return;
                        }
                        children.push(child);
                    }

                    let mut frame_idx: u32 = 0;
                    let mut buf = vec![0u8; frame_bytes];
                    let mut free_pixels = Vec::new();
                    let frame_period = std::time::Duration::from_nanos(1_000_000_000 / 30);
                    let mut next_frame_at: Option<std::time::Instant> = None;
                    while !stop_v.load(Ordering::Relaxed) {
                        let mut got = 0;
                        while got < frame_bytes && !stop_v.load(Ordering::Relaxed) {
                            match stdout.read(&mut buf[got..]) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => got += n,
                            }
                        }
                        if got < frame_bytes {
                            break;
                        }
                        // Pace delivery at 30 fps so HLS segment bursts play
                        // smoothly instead of fast-forwarding then freezing
                        // while waiting for the next segment.
                        let now = std::time::Instant::now();
                        match next_frame_at {
                            None => next_frame_at = Some(now + frame_period),
                            Some(target) => {
                                if target > now {
                                    let wait = target - now;
                                    if wait <= std::time::Duration::from_millis(100) {
                                        std::thread::sleep(wait);
                                    }
                                    next_frame_at = Some(target + frame_period);
                                } else if now.duration_since(target) > std::time::Duration::from_millis(250) {
                                    next_frame_at = Some(now + frame_period);
                                } else {
                                    next_frame_at = Some(target + frame_period);
                                }
                            }
                        }
                        let push_dvr = frame_idx % DVR_STRIDE == 0;
                        frame_idx = frame_idx.wrapping_add(1);
                        free_pixels.clear();
                        free_pixels.extend(buf.chunks_exact(3).map(|rgb| egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2])));
                        let image = Arc::new(egui::ColorImage::new([LIVE_W, LIVE_H], free_pixels));
                        let old = dvr_v.lock().unwrap().push_video(image, &buf, push_dvr);
                        free_pixels = old.and_then(|img| Arc::try_unwrap(img).ok()).map_or_else(Vec::new, |img| img.pixels);
                        has_v.store(true, Ordering::Release);
                        ctx_v.request_repaint();
                    }
                    if !stop_v.load(Ordering::Relaxed) && !has_v.load(Ordering::Acquire) {
                        let msg = diagnostic.lock().unwrap().take()
                            .unwrap_or_else(|| "Stream ended or could not be decoded.".into());
                        *err_v.lock().unwrap() = Some(msg);
                        ctx_v.request_repaint();
                    }
                });

                let (audio_tx, audio_rx) = std::sync::mpsc::sync_channel::<Option<Vec<f32>>>(16);
                let dvr_a = dvr.clone();
                let stop_a = stop.clone();
                // Drain FFmpeg's audio pipe without ever blocking on PulseAudio:
                // otherwise the synchronous `tee` muxer stalls video & HLS reads.
                let _ = std::thread::Builder::new().name("live-audio-read".into()).spawn(move || {
                    let mut raw = vec![0u8; AUDIO_CHUNK_SAMPLES * 4];
                    while !stop_a.load(Ordering::Relaxed) {
                        let mut got = 0;
                        while got < raw.len() && !stop_a.load(Ordering::Relaxed) {
                            match audio_input.read(&mut raw[got..]) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => got += n,
                            }
                        }
                        if got < 8 {
                            break;
                        }
                        let count = got / 4;
                        let mut chunk = Vec::with_capacity(count);
                        for ch in raw[..count * 4].chunks_exact(4) {
                            chunk.push(f32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]));
                        }
                        let to_play = dvr_a.lock().unwrap().push_audio_and_pick(chunk);
                        // Non-blocking send: drop if PulseAudio is temporarily behind.
                        let _ = audio_tx.try_send(to_play);
                    }
                });

                let muted_p = muted.clone();
                let vol_p = volume.clone();
                let stop_p = stop.clone();
                let _ = std::thread::Builder::new().name("live-audio-play".into()).spawn(move || {
                    let mut out = None;
                    let mut attempted_audio = false;
                    let mut scaled = vec![0.0f32; AUDIO_CHUNK_SAMPLES];
                    let mut was_paused = false;
                    while !stop_p.load(Ordering::Relaxed) {
                        let Ok(to_play) = audio_rx.recv() else { break };
                        if stop_p.load(Ordering::Relaxed) {
                            break;
                        }
                        if !attempted_audio {
                            attempted_audio = true;
                            match crate::pulse::Output::new(AUDIO_RATE, AUDIO_CHANNELS as u16)
                                .and_then(|mut output| {
                                    output.start()?;
                                    Ok(output)
                                })
                            {
                                Ok(output) => out = Some(output),
                                Err(error) => tracing::warn!("broadcast audio output unavailable: {error}"),
                            }
                        }
                        let Some(output) = &mut out else { continue };
                        match to_play {
                            None => {
                                if !was_paused {
                                    let _ = output.reset();
                                    let _ = output.start();
                                    was_paused = true;
                                }
                            }
                            Some(samples) => {
                                was_paused = false;
                                let v = f32::from_bits(vol_p.load(Ordering::Relaxed)).clamp(0.0, 1.0);
                                let gain = if muted_p.load(Ordering::Relaxed) { 0.0 } else { v * v };
                                scaled.clear();
                                scaled.extend(samples.iter().map(|s| s * gain));
                                if let Err(e) = output.write(&scaled) {
                                    tracing::warn!("broadcast audio write failed: {e}");
                                    out = None;
                                }
                            }
                        }
                    }
                });
            } else {
                *error.lock().unwrap() = Some("Could not open broadcast audio pipe.".into());
            }
        }

        #[cfg(windows)]
        {
            let _ = (ctx, validated_url, auth_params);
            *error.lock().unwrap() = Some("Live broadcast playback is currently supported on Linux (ffmpeg).".into());
        }

        Self {
            dvr,
            uploaded_rev: u64::MAX,
            texture: None,
            muted,
            volume,
            stop,
            error,
            has_frame,
            #[cfg(not(windows))]
            procs,
        }
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    pub fn toggle_mute(&self) {
        self.muted.fetch_xor(true, Ordering::Relaxed);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed)).clamp(0.0, 1.0)
    }

    pub fn set_volume(&self, v: f32) {
        let clamped = v.clamp(0.0, 1.0);
        self.volume.store(clamped.to_bits(), Ordering::Relaxed);
        if clamped > 0.0 && self.is_muted() {
            self.muted.store(false, Ordering::Relaxed);
        }
    }

    pub fn is_paused(&self) -> bool {
        self.dvr.lock().unwrap().paused
    }

    pub fn toggle_pause(&self) {
        let mut st = self.dvr.lock().unwrap();
        st.paused = !st.paused;
        st.revision = st.revision.wrapping_add(1);
    }

    pub fn behind_secs(&self) -> f32 {
        self.dvr.lock().unwrap().behind
    }

    pub fn buffered_secs(&self) -> f32 {
        self.dvr.lock().unwrap().buffered_secs()
    }

    pub fn is_live(&self) -> bool {
        let st = self.dvr.lock().unwrap();
        !st.paused && st.is_live()
    }

    pub fn rewind(&self, secs: f32) {
        let mut st = self.dvr.lock().unwrap();
        st.behind = (st.behind + secs.max(0.0)).min(st.buffered_secs());
        st.revision = st.revision.wrapping_add(1);
    }

    pub fn forward(&self, secs: f32) {
        let mut st = self.dvr.lock().unwrap();
        st.behind = (st.behind - secs.max(0.0)).max(0.0);
        st.revision = st.revision.wrapping_add(1);
    }

    pub fn seek_behind(&self, secs: f32) {
        let mut st = self.dvr.lock().unwrap();
        st.behind = secs.clamp(0.0, st.buffered_secs());
        st.revision = st.revision.wrapping_add(1);
    }

    pub fn jump_to_live(&self) {
        let mut st = self.dvr.lock().unwrap();
        st.behind = 0.0;
        st.paused = false;
        st.revision = st.revision.wrapping_add(1);
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    pub fn has_frame(&self) -> bool {
        self.has_frame.load(Ordering::Acquire)
    }

    /// Upload the active live or DVR frame to the egui texture and return its ID.
    pub fn texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureId> {
        let maybe_img = {
            let st = self.dvr.lock().unwrap();
            if st.revision == self.uploaded_rev {
                None
            } else {
                st.current_frame().map(|img| (st.revision, img))
            }
        };
        if let Some((rev, img)) = maybe_img {
            self.uploaded_rev = rev;
            match &mut self.texture {
                Some(h) if h.size() == img.size => h.set(img, egui::TextureOptions::LINEAR),
                _ => {
                    self.texture = Some(ctx.load_texture("live-broadcast", img, egui::TextureOptions::LINEAR));
                }
            }
        }
        self.texture.as_ref().map(|h| h.id())
    }
}

impl Drop for LivePlayer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        #[cfg(not(windows))]
        if let Ok(mut list) = self.procs.lock() {
            for mut child in list.drain(..) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    #[ignore = "requires local ffmpeg HLS fixtures"]
    fn ffmpeg_broadcast_handles_audio_and_silent_feeds() {
        let ctx = egui::Context::default();
        for (env, expect_audio) in [("AURORA_TEST_HLS_AV", true), ("AURORA_TEST_HLS_SILENT", false)] {
            let player = LivePlayer::start(&ctx, std::env::var(env).expect(env));
            let start = std::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_secs(8) {
                assert!(player.error().is_none(), "{:?}", player.error());
                if player.has_frame() && (!expect_audio || !player.dvr.lock().unwrap().audio.is_empty()) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            assert!(player.has_frame());
            let dvr = player.dvr.lock().unwrap();
            assert_eq!(dvr.live_frame.as_ref().unwrap().size, [LIVE_W, LIVE_H]);
            assert_eq!(!dvr.audio.is_empty(), expect_audio);
            assert_eq!(player.procs.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn dvr_rewind_pause_and_jump_to_live() {
        let mut st = DvrState::default();
        let push = |st: &mut DvrState, i| {
            let rgb = vec![i; LIVE_W * LIVE_H * 3];
            st.push_video(Arc::new(egui::ColorImage::from_rgb([LIVE_W, LIVE_H], &rgb)), &rgb, true);
        };
        for i in 0..30u8 {
            push(&mut st, i);
            let _ = st.push_audio_and_pick(vec![i as f32; 8]);
        }
        assert!(st.buffered_secs() >= 2.8);
        assert!(st.is_live());
        // Live returns full-res latest frame.
        let frame = st.current_frame().unwrap();
        assert_eq!(frame.size, [LIVE_W, LIVE_H]);
        assert_eq!(frame.pixels[0].r(), 29);

        // Rewind 1.0s (10 chunks back) returns downscaled DVR frame & matching audio.
        st.behind = 1.0;
        assert!(!st.is_live());
        let frame = st.current_frame().unwrap();
        assert_eq!(frame.size, [DVR_W, DVR_H]);
        assert_eq!(frame.pixels[0].r(), 19);

        // Pause freezes frame and silences audio while live continues buffering.
        st.paused = true;
        push(&mut st, 30);
        assert!(st.push_audio_and_pick(vec![30.0; 8]).is_none());
        assert_eq!(st.current_frame().unwrap().pixels[0].r(), 19);
    }

    #[cfg(not(windows))]
    #[test]
    fn rewrites_hls_auth_params_for_playlists_maps_and_segments() {
        let auth = "&token=ip=1.2.3.4~hmac=abc";
        let base = "http://127.0.0.1:42000";
        let mut playlists = vec!["https://cdn/master.m3u8?origin=x".to_string()];
        let master = concat!(
            "#EXTM3U\n",
            "#EXT-X-MEDIA:TYPE=AUDIO,URI=\"https://cdn/audio.m3u8?origin=x\"\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=1000\n",
            "https://cdn/video.m3u8?origin=x\n",
        );
        let rewritten_master = rewrite_hls_playlist(master, auth, base, &mut playlists);
        assert!(rewritten_master.contains("URI=\"http://127.0.0.1:42000/p/1.m3u8\""));
        assert!(rewritten_master.contains("\nhttp://127.0.0.1:42000/p/2.m3u8\n"));
        assert_eq!(playlists.len(), 3);

        let media = concat!(
            "#EXTM3U\n",
            "#EXT-X-MAP:URI=\"https://cdn/init/audio?origin=x\"\n",
            "#EXTINF:2.0,\n",
            "https://cdn/segment/audio/1/100/?origin=x\n",
        );
        let rewritten_media = rewrite_hls_playlist(media, auth, base, &mut playlists);
        assert!(rewritten_media.contains("URI=\"https://cdn/init/audio?origin=x&token=ip=1.2.3.4~hmac=abc\""));
        assert!(rewritten_media.contains("https://cdn/segment/audio/1/100/?origin=x&token=ip=1.2.3.4~hmac=abc"));
    }

    #[test]
    fn validates_live_hls_urls_against_ssrf_and_flag_injection() {
        assert!(validate_live_hls_url("https://video.akamai.steamstatic.com/hls/master.m3u8").is_ok());
        for bad in [
            "",
            "-f",
            "file:///etc/passwd",
            "http://video.akamai.steamstatic.com/hls/master.m3u8",
            "https://localhost/master.m3u8",
            "https://127.0.0.1/master.m3u8",
            "https://10.0.0.1/master.m3u8",
            "https://192.168.1.5/master.m3u8",
            "https://169.254.169.254/latest/meta-data",
            "https://[::1]/master.m3u8",
            "https://example.com/\nfoo",
        ] {
            assert!(validate_live_hls_url(bad).is_err(), "expected rejection for {bad:?}");
        }
    }
}

#[cfg(not(windows))]
fn append_auth(url: &str, auth: &str) -> String {
    if auth.is_empty() || url.contains("token=") {
        return url.to_string();
    }
    let clean = auth.trim_start_matches(['&', '?']);
    if url.contains('?') {
        format!("{url}&{clean}")
    } else {
        format!("{url}?{clean}")
    }
}

#[cfg(not(windows))]
fn register_playlist(playlists: &mut Vec<String>, url: &str) -> usize {
    if let Some(pos) = playlists.iter().position(|u| u == url) {
        pos
    } else {
        let idx = playlists.len();
        playlists.push(url.to_string());
        idx
    }
}

#[cfg(not(windows))]
fn rewrite_hls_playlist(body: &str, auth: &str, base_local: &str, playlists: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(body.len() + 512);
    for raw in body.lines() {
        let line = raw.trim_end_matches('\r');
        if (line.starts_with("#EXT-X-MEDIA:") || line.starts_with("#EXT-X-MAP:"))
            && let Some(uri_start) = line.find("URI=\"")
        {
            let val_start = uri_start + 5;
            if let Some(val_end_rel) = line[val_start..].find('"') {
                let val_end = val_start + val_end_rel;
                let uri = &line[val_start..val_end];
                let replacement = if uri.contains(".m3u8") || line.starts_with("#EXT-X-MEDIA:") {
                    let idx = register_playlist(playlists, uri);
                    format!("{base_local}/p/{idx}.m3u8")
                } else {
                    append_auth(uri, auth)
                };
                out.push_str(&line[..val_start]);
                out.push_str(&replacement);
                out.push_str(&line[val_end..]);
                out.push('\n');
                continue;
            }
        } else if line.starts_with("http://") || line.starts_with("https://") {
            if line.contains(".m3u8") {
                let idx = register_playlist(playlists, line);
                out.push_str(&format!("{base_local}/p/{idx}.m3u8\n"));
            } else {
                out.push_str(&append_auth(line, auth));
                out.push('\n');
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(not(windows))]
fn start_hls_auth_proxy(master_url: String, auth_params: String, stop: Arc<AtomicBool>) -> std::io::Result<String> {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    let base_local = format!("http://127.0.0.1:{port}");
    let entry = format!("{base_local}/p/0.m3u8");

    std::thread::Builder::new().name("live-hls-proxy".into()).spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("Mozilla/5.0 Aurora")
            .build()
            .unwrap_or_default();
        let mut playlists = vec![master_url];

        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                    let mut req_buf = [0u8; 2048];
                    let n = stream.read(&mut req_buf).unwrap_or(0);
                    let req = std::str::from_utf8(&req_buf[..n]).unwrap_or("");
                    let path = req.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("");
                    let idx = path
                        .strip_prefix("/p/")
                        .and_then(|s| s.strip_suffix(".m3u8"))
                        .and_then(|s| s.parse::<usize>().ok());
                    let Some(upstream) = idx.and_then(|i| playlists.get(i)).cloned() else {
                        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        continue;
                    };
                    let upstream_auth = append_auth(&upstream, &auth_params);
                    let fetched = rt.block_on(async {
                        let resp = http.get(&upstream_auth).send().await.ok()?;
                        if !resp.status().is_success() {
                            return None;
                        }
                        resp.text().await.ok()
                    });
                    match fetched {
                        Some(body) => {
                            let rewritten = rewrite_hls_playlist(&body, &auth_params, &base_local, &mut playlists);
                            let header = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                rewritten.len()
                            );
                            let _ = stream.write_all(header.as_bytes());
                            let _ = stream.write_all(rewritten.as_bytes());
                        }
                        None => {
                            let _ = stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break,
            }
        }
    })?;

    Ok(entry)
}
