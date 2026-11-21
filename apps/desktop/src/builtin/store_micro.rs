//! Store: microtrailers on hover, adapted from upstream work by `madonchik123`
//! (PR #64 `2e6e660` & PR #113 `c23b851`).
//!
//! A card's art swaps into the game's short silent clip while the pointer rests
//! on it. Guarded with `video::supported()` and failure tracking so Linux/Windows
//! hosts without a video backend keep static art cleanly without memory churn.

use super::{Load, Store};
use egui::{Color32, CornerRadius, Rect, Ui, pos2};
use model::AppId;
use std::collections::HashSet;

/// How long the pointer rests on a card before its clip starts (seconds).
const DELAY: f64 = 0.45;

#[derive(Default)]
pub(super) struct Micro {
    /// The card under the pointer, and since when.
    hovered: Option<(AppId, f64)>,
    /// At most one active decoded clip at a time across the entire Store.
    player: Option<(AppId, video::Player)>,
    /// Clips that failed to decode or download this session (stay on static art).
    failed: HashSet<AppId>,
    /// A card was hovered this frame (else the active clip is dropped).
    seen: bool,
}

impl Store {
    /// Draw a hovering microtrailer over `img` when the pointer rests on card `id`.
    pub(super) fn micro(&mut self, ui: &Ui, id: AppId, img: Rect, corner: CornerRadius, hovered: bool) {
        if !hovered || !video::supported() || self.micro.failed.contains(&id) {
            return;
        }
        let now = ui.input(|i| i.time);
        self.micro.seen = true;
        if self.micro.hovered.is_none_or(|(a, _)| a != id) {
            self.micro.hovered = Some((id, now));
            self.micro.player = None;
        }
        let since = self.micro.hovered.map(|(_, t)| t).unwrap_or(now);
        if now - since < DELAY {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(DELAY - (now - since) + 0.01));
            return;
        }
        if self.micro.player.is_none() {
            let Some(info) = self.browse_info(id) else {
                // Only keep checking while BrowseInfo is loading, not after a failure.
                if self.browse.get(&id).is_some_and(|s| matches!(&*s.lock().unwrap(), Load::Loading)) {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
                }
                return;
            };
            let Some(url) = info.trailers.iter().find_map(|t| t.micro_mp4.clone()) else { return };
            let opts = video::Options { autoplay: true, muted: true, volume: 0.0, looping: true, width: 480 };
            self.micro.player = Some((id, video::Player::new(&self.rt, ui.ctx(), video::Source::Mp4(url), opts)));
        }
        let Some((_, p)) = self.micro.player.as_mut() else { return };
        p.poll(ui.ctx());
        if p.status() == video::Status::Failed {
            self.micro.failed.insert(id);
            self.micro.player = None;
            return;
        }
        if let Some((tex, size)) = p.texture() {
            // Cover the card art (crop sides or top/bottom cleanly to preserve aspect).
            let (want, have) = (img.width() / img.height().max(1.0), size.x / size.y.max(1.0));
            let uv = if have > want {
                let w = want / have;
                Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
            } else {
                let h = have / want;
                Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
            };
            let painter = ui.painter_at(img);
            painter.add(egui::epaint::RectShape::filled(img, corner, Color32::WHITE).with_texture(tex, uv));
        }
    }

    /// Drop any clip whose card is no longer hovered.
    pub(super) fn micro_end_frame(&mut self) {
        if !std::mem::take(&mut self.micro.seen) {
            self.micro.hovered = None;
            self.micro.player = None;
        }
    }
}
