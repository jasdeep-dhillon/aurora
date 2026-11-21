//! Store: search-as-you-type game suggestions for the Store search box,
//! adapted from upstream work by `madonchik123` (PR #116 `918e2b2`).

use super::{Load, Shared, Store, View};
use egui::{CornerRadius, Margin, Rect, Sense, Stroke, Ui, pos2, vec2};
use extensions::Ctx;
use i18n::t;
use store::Item;
use ui::{Theme, icons, widgets as w};

/// How long typing has to pause before asking Steam.
const PAUSE: f64 = 0.3;
/// How many games to show.
const SHOWN: usize = 8;

#[derive(Default)]
pub(crate) struct Suggest {
    /// The text and when it last changed.
    typed: String,
    since: f64,
    /// The text asked about, and Steam's answer.
    asked: Option<(String, Shared<Vec<Item>>)>,
    /// The last answer that arrived (shown while a newer one loads).
    last: Option<(String, Vec<Item>)>,
    /// The store's own search box: the dropdown is open, this row is picked.
    pub open: bool,
    pub sel: Option<usize>,
}

impl Store {
    /// Games matching `query` (what's ready so far: `None` before the
    /// first answer). Asks Steam once typing has paused; content the user
    /// chose to hide is left out.
    pub(crate) fn suggestions_for(&mut self, ctx: &Ctx, query: &str, now: f64) -> Option<Vec<Item>> {
        let q = query.trim();
        if q.chars().count() < 2 || !self.sync_locale(ctx) {
            return None;
        }
        if self.suggest.typed != q {
            self.suggest.typed = q.to_string();
            self.suggest.since = now;
        }
        if now - self.suggest.since >= PAUSE && self.suggest.asked.as_ref().is_none_or(|(a, _)| a != q) {
            let (api, qq) = (self.api.clone(), q.to_string());
            let slot = self.fetch(async move { api.search(&qq).await });
            self.suggest.asked = Some((q.to_string(), slot));
        } else if now - self.suggest.since < PAUSE {
            self.egui.request_repaint_after(std::time::Duration::from_secs_f64(PAUSE - (now - self.suggest.since) + 0.02));
        }
        if let Some((a, slot)) = &self.suggest.asked {
            if let Load::Ready(v) = &*slot.lock().unwrap() {
                self.suggest.last = Some((a.clone(), v.clone()));
            }
        }
        let (_, items) = self.suggest.last.clone()?;
        let shown: Vec<Item> = items.into_iter().filter(|i| self.safe(i.app_id) != Some(false)).take(SHOWN).collect();
        self.flush_ratings();
        Some(shown)
    }

    /// Still waiting for Steam about `query`.
    pub(crate) fn suggestions_loading(&self, query: &str) -> bool {
        let q = query.trim();
        self.suggest.last.as_ref().is_none_or(|(a, _)| a != q) && q.chars().count() >= 2
    }

    /// The store's search box in the subnav bar, with its suggestion dropdown.
    pub(super) fn search_box(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, width: f32) {
        let now = ui.input(|i| i.time);
        let q = self.bar_query.trim().to_string();
        let items = if self.suggest.open { self.suggestions_for(ctx, &q, now).unwrap_or_default() } else { Vec::new() };
        // The rows: the games, then "See all results".
        let rows = items.len() + 1;
        // Up and down move through the rows while the field has focus
        // (consumed before TextEdit handles the arrow keys).
        if self.suggest.open && q.chars().count() >= 2 {
            let (down, up) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)));
            if down {
                self.suggest.sel = Some(next_row(self.suggest.sel, rows));
            }
            if up {
                self.suggest.sel = prev_row(self.suggest.sel);
            }
        }
        let before = self.bar_query.clone();
        let r = w::search(ui, t, &mut self.bar_query, t!("Search the store"), width);
        if self.bar_query != before {
            self.suggest.sel = None;
            self.suggest.open = true;
        }
        if r.gained_focus() {
            self.suggest.open = true;
        }
        let mut pick: Option<usize> = None;
        if r.lost_focus() {
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                pick = Some(self.suggest.sel.unwrap_or(rows - 1));
            } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.suggest.open = false;
            }
        }
        if self.suggest.open && q.chars().count() >= 2 {
            let loading = self.suggestions_loading(&q);
            let (clicked, outside) = suggestion_popup(ui.ctx(), t, r.rect, &q, &items, loading, &mut self.suggest.sel);
            if clicked.is_some() {
                pick = clicked;
            } else if outside && !r.has_focus() {
                self.suggest.open = false;
            }
        }
        let q = self.bar_query.trim().to_string();
        if let Some(i) = pick.filter(|_| !q.is_empty()) {
            self.suggest.open = false;
            self.suggest.sel = None;
            match items.get(i) {
                Some(item) => {
                    self.bar_query.clear();
                    self.open(View::App(item.app_id));
                }
                None => self.start_search(q),
            }
        }
    }
}

pub(crate) fn next_row(sel: Option<usize>, rows: usize) -> usize {
    sel.map_or(0, |i| (i + 1).min(rows.saturating_sub(1)))
}

pub(crate) fn prev_row(sel: Option<usize>) -> Option<usize> {
    sel.and_then(|i| i.checked_sub(1))
}

/// The dropdown under the Store search box (`anchor`): one row per game, then
/// "See all results for …". Kept inside the window. Returns a clicked row and
/// whether the pointer clicked outside the dropdown and search box.
pub(crate) fn suggestion_popup(ctx: &egui::Context, t: &Theme, anchor: Rect, query: &str, items: &[Item], loading: bool, sel: &mut Option<usize>) -> (Option<usize>, bool) {
    let screen = ctx.content_rect();
    let width = anchor.width().max(380.0).min((screen.width() - 16.0).max(200.0));
    let x = anchor.left().min(screen.right() - width - 8.0).max(screen.left() + 8.0);
    let mut clicked = None;
    let area = egui::Area::new(egui::Id::new("store-suggestions"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(x, anchor.bottom() + 6.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(t.surface)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(t.corner())
                .inner_margin(Margin::same(6))
                .shadow(egui::Shadow { offset: [0, 6], blur: 16, spread: 0, color: egui::Color32::from_black_alpha(90) })
                .show(ui, |ui| {
                    ui.set_width(width);
                    w::menu_spacing(ui);
                    if items.is_empty() {
                        let text = if loading { t!("Searching…") } else { t!("No games match.") };
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            ui.label(w::text(text, 12.5, t.text_faint));
                        });
                        ui.add_space(4.0);
                    }
                    for (i, item) in items.iter().enumerate() {
                        let r = suggestion_row(ui, t, item, *sel == Some(i));
                        if r.hovered() && ui.input(|inp| inp.pointer.delta() != egui::Vec2::ZERO) {
                            *sel = Some(i);
                        }
                        if r.clicked() {
                            clicked = Some(i);
                        }
                    }
                    let all = items.len();
                    let r = w::menu_row(ui, t, icons::SEARCH, &t!("See all results for “{q}”", q = query), *sel == Some(all));
                    if r.hovered() && ui.input(|inp| inp.pointer.delta() != egui::Vec2::ZERO) {
                        *sel = Some(all);
                    }
                    if r.clicked() {
                        clicked = Some(all);
                    }
                });
        });
    let outside = ctx.input(|i| i.pointer.any_click()) && !area.response.hovered() && !ctx.pointer_hover_pos().is_some_and(|p| anchor.contains(p));
    (clicked, outside)
}

/// One suggested game: small header capsule, truncated title, and price on the right.
pub(crate) fn suggestion_row(ui: &mut Ui, t: &Theme, item: &Item, selected: bool) -> egui::Response {
    let h = 48.0;
    let width = if ui.is_sizing_pass() { 340.0 } else { ui.available_width().max(160.0) };
    let (rect, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
    if ui.is_rect_visible(rect) {
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || selected, 0.08);
        if hover > 0.0 {
            let fill = if selected { w::active_fill(t) } else { t.surface_hover };
            ui.painter().rect_filled(rect, CornerRadius::same(t.radius.saturating_sub(2).max(4)), fill.gamma_multiply(hover));
        }
        let pic_h = h - 12.0;
        let pic = Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - pic_h / 2.0), vec2((pic_h * 460.0 / 215.0).round(), pic_h));
        w::image_or_skeleton(ui, t, pic, &item.header, t.corner_small());
        let text_c = if selected { t.text } else { w::mix(t.text_dim, t.text, 0.65 + 0.35 * hover) };
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(pos2((rect.right() - 150.0).max(pic.right() + 24.0), rect.top()), pos2(rect.right() - 10.0, rect.bottom())))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        let price_left = if item.price.final_ == Some(0) {
            right.label(w::text_medium(t!("Free"), 13.0, text_c)).rect.left()
        } else if item.price.final_.is_some() {
            w::price_tag(&mut right, t, &item.price.label(), item.price.original_label().as_deref(), item.price.discount);
            right.min_rect().left()
        } else {
            rect.right() - 10.0
        };
        let x = pic.right() + 10.0;
        let max_w = (price_left - 10.0 - x).max(16.0);
        let mut job = egui::text::LayoutJob::single_section(item.name.clone(), egui::TextFormat::simple(w::font_medium(13.5), text_c));
        job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let g = ui.painter().layout_job(job);
        ui.painter().galley(pos2(x, rect.center().y - g.size().y / 2.0), g, text_c);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::{next_row, prev_row};

    #[test]
    fn keyboard_selection_moves_safely_within_bounds() {
        assert_eq!(next_row(None, 4), 0);
        assert_eq!(next_row(Some(0), 4), 1);
        assert_eq!(next_row(Some(3), 4), 3);
        assert_eq!(prev_row(None), None);
        assert_eq!(prev_row(Some(0)), None);
        assert_eq!(prev_row(Some(2)), Some(1));
    }
}
