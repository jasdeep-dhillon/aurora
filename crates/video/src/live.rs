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

struct DvrState {
    /// Latest full-res 960×540 RGB24 frame for live playback.
    live_frame: Option<Vec<u8>>,
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

    fn push_video(&mut self, rgb: Vec<u8>, push_dvr: bool) {
        if push_dvr {
            let small = downsample_2x_rgb(&rgb, LIVE_W, LIVE_H);
            if self.frames.len() >= DVR_MAX_CHUNKS {
                self.frames.pop_front();
            }
            self.frames.push_back(small);
            if self.paused {
                // Keep the frozen frame pinned as new live frames arrive.
                self.behind = (self.behind + 1.0 / DVR_FPS as f32).min(self.buffered_secs());
            } else {
                self.clamp_behind();
            }
        }
        self.live_frame = Some(rgb);
        if !self.paused {
            self.revision = self.revision.wrapping_add(1);
        }
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

    fn current_frame(&self) -> Option<([usize; 2], &[u8])> {
        if self.is_live() && !self.paused {
            if let Some(f) = self.live_frame.as_deref() {
                return Some(([LIVE_W, LIVE_H], f));
            }
        }
        if !self.frames.is_empty() {
            let back_idx = (self.behind * DVR_FPS as f32).round() as usize;
            let idx = self.frames.len().saturating_sub(1).saturating_sub(back_idx);
            if let Some(f) = self.frames.get(idx) {
                return Some(([DVR_W, DVR_H], f.as_slice()));
            }
        }
        self.live_frame.as_deref().map(|f| ([LIVE_W, LIVE_H], f))
    }
}

/// Fast 2×2 nearest downsample of an RGB24 frame (`w×h` -> `(w/2)×(h/2)`).
fn downsample_2x_rgb(src: &[u8], w: usize, h: usize) -> Vec<u8> {
    let dw = w / 2;
    let dh = h / 2;
    let mut dst = vec![0u8; dw * dh * 3];
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
    dst
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
        let dvr = Arc::new(Mutex::new(DvrState::default()));
        let muted = Arc::new(AtomicBool::new(false));
        let volume = Arc::new(AtomicU32::new(0.8_f32.to_bits()));
        let stop = Arc::new(AtomicBool::new(false));
        let error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let has_frame = Arc::new(AtomicBool::new(false));

        #[cfg(not(windows))]
        let procs: Arc<Mutex<Vec<std::process::Child>>> = Arc::new(Mutex::new(Vec::new()));

        #[cfg(not(windows))]
        {
            use std::io::Read;
            use std::process::{Command, Stdio};

            let bin = std::env::var("STEAMINRUST_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());

            // 1. Video stream thread: live HLS -> 960x540 RGB24 frames @ 30fps.
            {
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
                            "-re",
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
                            "-an",
                            "-vf",
                            &vf,
                            "-r",
                            "30",
                            "-pix_fmt",
                            "rgb24",
                            "-f",
                            "rawvideo",
                            "-",
                        ])
                        .stdin(Stdio::null())
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
                        return;
                    };
                    let stderr_pipe = child.stderr.take();
                    procs_v.lock().unwrap().push(child);

                    let mut frame_idx: u32 = 0;
                    while !stop_v.load(Ordering::Relaxed) {
                        let mut buf = vec![0u8; frame_bytes];
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
                        let push_dvr = frame_idx % DVR_STRIDE == 0;
                        frame_idx = frame_idx.wrapping_add(1);
                        dvr_v.lock().unwrap().push_video(buf, push_dvr);
                        has_v.store(true, Ordering::Release);
                        ctx_v.request_repaint();
                    }
                    if !stop_v.load(Ordering::Relaxed) && !has_v.load(Ordering::Acquire) {
                        let mut msg = String::new();
                        if let Some(mut se) = stderr_pipe {
                            let _ = se.read_to_string(&mut msg);
                        }
                        let msg = msg.trim();
                        *err_v.lock().unwrap() = Some(if msg.is_empty() {
                            "Stream ended or could not be decoded.".into()
                        } else {
                            msg.lines().next().unwrap_or(msg).to_string()
                        });
                        ctx_v.request_repaint();
                    }
                });
            }

            // 2. Audio stream thread: live HLS -> 48kHz stereo f32le -> PulseAudio/PipeWire.
            {
                let bin_a = bin;
                let url_a = url;
                let dvr_a = dvr.clone();
                let muted_a = muted.clone();
                let vol_a = volume.clone();
                let stop_a = stop.clone();
                let procs_a = procs.clone();
                let _ = std::thread::Builder::new().name("live-audio".into()).spawn(move || {
                    let mut out = match crate::pulse::Output::new(AUDIO_RATE, AUDIO_CHANNELS as u16) {
                        Ok(o) => o,
                        Err(e) => {
                            tracing::warn!("broadcast audio output unavailable: {e}");
                            return;
                        }
                    };
                    if let Err(e) = out.start() {
                        tracing::warn!("broadcast audio start failed: {e}");
                        return;
                    }
                    let Ok(mut child) = Command::new(&bin_a)
                        .args([
                            "-hide_banner",
                            "-loglevel",
                            "error",
                            "-nostdin",
                            "-re",
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
                            &url_a,
                            "-vn",
                            "-f",
                            "f32le",
                            "-ar",
                            "48000",
                            "-ac",
                            "2",
                            "-",
                        ])
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::null())
                        .spawn()
                    else {
                        return;
                    };
                    let Some(mut stdout) = child.stdout.take() else {
                        let _ = child.kill();
                        return;
                    };
                    procs_a.lock().unwrap().push(child);

                    let mut raw = vec![0u8; AUDIO_CHUNK_SAMPLES * 4];
                    let mut scaled = vec![0.0f32; AUDIO_CHUNK_SAMPLES];
                    let mut was_paused = false;
                    while !stop_a.load(Ordering::Relaxed) {
                        let mut got = 0;
                        while got < raw.len() && !stop_a.load(Ordering::Relaxed) {
                            match stdout.read(&mut raw[got..]) {
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
                        match to_play {
                            None => {
                                if !was_paused {
                                    let _ = out.reset();
                                    let _ = out.start();
                                    was_paused = true;
                                }
                            }
                            Some(samples) => {
                                was_paused = false;
                                let v = f32::from_bits(vol_a.load(Ordering::Relaxed)).clamp(0.0, 1.0);
                                let gain = if muted_a.load(Ordering::Relaxed) { 0.0 } else { v * v };
                                scaled.clear();
                                scaled.extend(samples.iter().map(|s| s * gain));
                                if let Err(e) = out.write(&scaled) {
                                    tracing::warn!("broadcast audio write failed: {e}");
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        }

        #[cfg(windows)]
        {
            let _ = (ctx, url);
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
                st.current_frame().map(|(size, rgb)| {
                    (st.revision, egui::ColorImage::from_rgb(size, rgb))
                })
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

    #[test]
    fn dvr_rewind_pause_and_jump_to_live() {
        let mut st = DvrState::default();
        for i in 0..30u8 {
            st.push_video(vec![i; LIVE_W * LIVE_H * 3], true);
            let _ = st.push_audio_and_pick(vec![i as f32; 8]);
        }
        assert!(st.buffered_secs() >= 2.8);
        assert!(st.is_live());
        // Live returns full-res latest frame.
        let (sz, data) = st.current_frame().unwrap();
        assert_eq!(sz, [LIVE_W, LIVE_H]);
        assert_eq!(data[0], 29);

        // Rewind 1.0s (10 chunks back) returns downscaled DVR frame & matching audio.
        st.behind = 1.0;
        assert!(!st.is_live());
        let (sz, data) = st.current_frame().unwrap();
        assert_eq!(sz, [DVR_W, DVR_H]);
        assert_eq!(data[0], 19);

        // Pause freezes frame and silences audio while live continues buffering.
        st.paused = true;
        st.push_video(vec![30; LIVE_W * LIVE_H * 3], true);
        assert!(st.push_audio_and_pick(vec![30.0; 8]).is_none());
        let (_, data) = st.current_frame().unwrap();
        assert_eq!(data[0], 19);
    }
}
