//! Store: filtered browsing, wishlist (sort, filter, list/grid, drag & rank reorder),
//! cart (with gifting), and price history.
//! Enriched rows, Clear (n) filter header, skeleton placeholders, and wishlist
//! rank reordering adapted from upstream work by `madonchik123` (PR #68, #74, #108, #116).

use super::{Load, Shared, Store, View};
use client::Command;
use egui::{Align, Align2, Color32, Layout, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use extensions::Ctx;
use i18n::t;
use model::{AppId, SteamId};
use store::query::{QueryItem, Sort};
use ui::{Theme, icons, widgets as w};

/// Popular tags offered as quick filters (Steam tag ids).
const TAGS: &[u32] = &[19, 21, 122, 9, 597, 599, 492, 1664, 1667, 1774, 1695, 3810, 1662, 4182, 3859, 1685, 701, 699, 1742, 3964, 128, 3871];

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum WishSort {
    #[default]
    Rank,
    Discount,
    PriceLow,
    PriceHigh,
    Name,
    Released,
    Reviews,
}

impl WishSort {
    const ALL: [WishSort; 7] = [
        WishSort::Rank,
        WishSort::Discount,
        WishSort::PriceLow,
        WishSort::PriceHigh,
        WishSort::Name,
        WishSort::Released,
        WishSort::Reviews,
    ];

    fn label(self) -> &'static str {
        match self {
            WishSort::Rank => "Your rank",
            WishSort::Discount => "Discount",
            WishSort::PriceLow => "Price: low to high",
            WishSort::PriceHigh => "Price: high to low",
            WishSort::Name => "Name",
            WishSort::Released => "Release date",
            WishSort::Reviews => "Reviews",
        }
    }
}

#[derive(Default)]
pub struct WishView {
    pub sort: WishSort,
    pub grid: bool,
    pub query: String,
    /// 0 = Any, 1 = Windows, 2 = macOS, 3 = Linux.
    pub platform: u8,
    /// Max price in cents.
    pub max_price: Option<u64>,
    pub on_sale: bool,
    /// Rows can be dragged and renumbered (sorted by "Your rank", as a list).
    reorderable: bool,
    /// The game being dragged.
    drag: Option<AppId>,
    /// This frame's rows: (game, rank from 0, row rect) for drop targeting.
    rows: Vec<(AppId, usize, Rect)>,
    /// Inline rank editor: (game, text, just opened: autofocus once).
    rank_edit: Option<(AppId, String, bool)>,
    /// Optimistic order while Steam saves a move (and local order in `--demo`).
    pub pending: Option<Vec<AppId>>,
}

pub struct Gift {
    pub app: AppId,
    pub package: u32,
    pub name: String,
    pub to: Option<SteamId>,
    /// The cart line this picks a friend for (None: add a new gift copy).
    pub line: Option<u64>,
    /// Filter for the friend list.
    pub search: String,
    /// Region checks per friend ("can Steam deliver this gift to them?").
    pub checks: std::collections::HashMap<SteamId, Shared<store::gift::GiftCheck>>,
}

fn snapshot<T: Clone>(slot: &Shared<T>) -> Option<Result<T, String>> {
    match &*slot.lock().unwrap() {
        Load::Loading => None,
        Load::Ready(v) => Some(Ok(v.clone())),
        Load::Failed(e) => Some(Err(e.clone())),
    }
}

impl Store {
    // ---------- browse ----------

    pub(super) fn browse_page(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let mut f = self.filters.clone();
        f.hidden_descriptors = self.hidden.iter().copied().collect();
        if self.query.as_ref().is_none_or(|(key, page, _)| *key != f || *page != self.browse_page) {
            let (api, ff, start) = (self.api.clone(), f.clone(), self.browse_page * 48);
            let slot = self.fetch(async move { api.query(&ff, start, 48).await.map(std::sync::Arc::new) });
            self.query = Some((f.clone(), self.browse_page, slot));
        }
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 24.0;
            ui.vertical(|ui| {
                ui.set_width(250.0);
                self.filter_panel(ui, t);
            });
            ui.vertical(|ui| {
                let Some((_, _, slot)) = &self.query else { return };
                match snapshot(slot) {
                    None => w::loading(ui, t, t!("Searching…")),
                    Some(Err(e)) => w::empty(ui, t, icons::WARNING, t!("Search failed"), &e),
                    Some(Ok(result)) => {
                        let (items, total) = result.as_ref();
                        let owned = |id: &AppId| self.owned.contains(id);
                        let items: Vec<_> = items.iter().filter(|i| !self.hide_owned || !owned(&i.app_id)).collect();
                        ui.label(w::text(t!("{n} results", n = total), 13.5, t.text_dim));
                        ui.add_space(8.0);
                        for it in &items {
                            if self.result_row(ui, t, ctx, it, None) {
                                self.open(View::App(it.app_id));
                            }
                            ui.add_space(6.0);
                        }
                        let pages = total.div_ceil(48).max(1) as u32;
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(self.browse_page > 0, |ui| {
                                if w::button(ui, t, Some(icons::BACK), t!("Previous"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                    self.browse_page -= 1;
                                }
                            });
                            ui.label(w::text(t!("Page {page} of {pages}", page = self.browse_page + 1, pages = pages), 13.0, t.text_dim));
                            ui.add_enabled_ui(self.browse_page + 1 < pages, |ui| {
                                if w::button(ui, t, Some(icons::CARET_RIGHT), t!("Next"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                    self.browse_page += 1;
                                }
                            });
                        });
                    }
                }
            });
        });
    }

    fn filter_panel(&mut self, ui: &mut Ui, t: &Theme) {
        let before = self.filters.clone();
        let active = browse_filter_count(&self.filters, self.hide_owned);
        let mut hide_owned = self.hide_owned;
        let reset = filter_column(ui, t, active, |ui| {
            let f = &mut self.filters;
            ui.label(w::caption_text(t, t!("SORT BY")));
            egui::ComboBox::from_id_salt("browse-sort").selected_text(i18n::tr_dyn(f.sort.label())).width(200.0).show_ui(ui, |ui| {
                for s in Sort::ALL {
                    ui.selectable_value(&mut f.sort, s, i18n::tr_dyn(s.label()));
                }
            });
            ui.add_space(10.0);
            ui.label(w::caption_text(t, t!("PRICE")));
            let options: [(Option<u32>, bool, &str); 6] = [
                (None, false, "Any price"),
                (None, true, "Free"),
                (Some(500), false, "Under $5"),
                (Some(1000), false, "Under $10"),
                (Some(2000), false, "Under $20"),
                (Some(4000), false, "Under $40"),
            ];
            for (max, free, label) in options {
                let on = f.max_price == max && f.only_free == free;
                if w::radio(ui, t, on, i18n::tr_dyn(label)).clicked() {
                    f.max_price = max;
                    f.only_free = free;
                }
            }
            w::check(ui, t, &mut f.only_discounted, t!("Special offers only"));
            ui.add_space(10.0);
            ui.label(w::caption_text(t, t!("REVIEWS")));
            for (min, label) in [(None, "Any reviews"), (Some(7), "Mostly positive and up"), (Some(8), "Very positive and up")] {
                if w::radio(ui, t, f.min_review_score == min, i18n::tr_dyn(label)).clicked() {
                    f.min_review_score = min;
                }
            }
            ui.add_space(10.0);
            ui.label(w::caption_text(t, t!("TYPE")));
            w::check(ui, t, &mut f.games, t!("Games"));
            w::check(ui, t, &mut f.dlc, t!("DLC"));
            w::check(ui, t, &mut f.software, t!("Software"));
            w::check(ui, t, &mut f.demos, t!("Demos"));
            w::check(ui, t, &mut f.soundtracks, t!("Soundtracks"));
            w::check(ui, t, &mut f.coming_soon, t!("Coming soon only"));
            ui.add_space(8.0);
            w::check(ui, t, &mut hide_owned, t!("Hide games I own"));
        });
        self.hide_owned = hide_owned;
        ui.add_space(10.0);
        w::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(w::caption_text(t, t!("TAGS")));
            // Any of Steam's tags, by name.
            let width = ui.available_width();
            w::search(ui, t, &mut self.tag_query, t!("Find a tag…"), width);
            let query = self.tag_query.trim().to_string();
            if query.chars().count() >= 2 {
                let found: Vec<(u32, String)> = self.find_tags(&query).into_iter().filter(|(id, _)| !TAGS.contains(id) && !self.filters.tags.contains(id)).take(12).collect();
                ui.add_space(6.0);
                if found.is_empty() {
                    ui.label(w::text(t!("No tags match."), 12.5, t.text_faint));
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for (id, name) in found {
                        if w::chip(ui, t, &format!("{}  {name}", icons::PLUS), false).clicked() {
                            self.filters.tags.push(id);
                            self.tag_query.clear();
                        }
                    }
                });
            }
            ui.add_space(8.0);
            // Active tags not in the quick list stay visible first (waiting for names as skeletons, never `#19`).
            let extra: Vec<u32> = self.filters.tags.iter().copied().filter(|tag| !TAGS.contains(tag)).collect();
            if !extra.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for tag in extra {
                        let Some(name) = self.tag_name(tag) else {
                            let (r, _) = ui.allocate_exact_size(vec2(72.0, 26.0), Sense::hover());
                            w::skeleton(ui, t, r, t.corner_small());
                            continue;
                        };
                        if w::chip(ui, t, &format!("{name}  {}", icons::X), true).on_hover_text(t!("Remove this tag")).clicked() {
                            self.filters.tags.retain(|x| *x != tag);
                        }
                    }
                });
                ui.add_space(6.0);
            }
            if !self.tag_names_ready() {
                for _ in 0..6 {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
                    w::skeleton(ui, t, r.shrink2(vec2(0.0, 3.0)), t.corner_small());
                }
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for tag in TAGS {
                        let Some(name) = self.tag_name(*tag) else { continue };
                        let on = self.filters.tags.contains(tag);
                        if w::chip(ui, t, &name, on).clicked() {
                            if on {
                                self.filters.tags.retain(|x| x != tag);
                            } else {
                                self.filters.tags.push(*tag);
                            }
                        }
                    }
                });
            }
            if !self.filters.tags.is_empty() && ui.link(w::text(t!("Clear tags"), 12.5, t.text_dim)).clicked() {
                self.filters.tags.clear();
            }
        });
        if reset {
            self.filters = store::query::Filters { sort: self.filters.sort, ..Default::default() };
            self.hide_owned = false;
            self.tag_query.clear();
        }
        if self.filters != before || reset {
            self.browse_page = 0;
        }
    }

    /// One game in a list (Browse or Wishlist): rank column when reorderable,
    /// header art, title, clickable tags, platforms/release date, review bar,
    /// price/discount expiry, Add to Cart, and Wishlist heart. Returns true when clicked.
    fn result_row(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, it: &QueryItem, rank: Option<(usize, usize)>) -> bool {
        let h = 144.0;
        let reorder = rank.is_some() && self.wish.reorderable;
        let sense = if reorder { Sense::click_and_drag() } else { Sense::click() };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), sense);
        if let (true, Some((n, _))) = (reorder, rank) {
            self.wish.rows.push((it.app_id, n, rect));
            if resp.drag_started() {
                self.wish.drag = Some(it.app_id);
                self.wish.rank_edit = None;
            }
        }
        if !ui.is_rect_visible(rect) {
            return false;
        }
        let dragging = self.wish.drag == Some(it.app_id);
        let hover = if dragging { 0.0 } else { ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1) };
        ui.painter().rect_filled(rect, t.corner_small(), w::mix(t.surface, t.surface_hover, hover));
        if dragging {
            ui.painter().rect_filled(rect, t.corner_small(), Color32::from_black_alpha(120));
        }
        let mut x_left = rect.left() + 8.0;
        if let Some((pos, total)) = rank {
            if reorder {
                let rank_rect = Rect::from_min_size(rect.min + vec2(4.0, 0.0), vec2(44.0, h));
                self.rank_column(ui, t, ctx, it.app_id, pos, total, rank_rect);
                x_left = rank_rect.right() + 4.0;
            }
        }
        let img = Rect::from_min_size(pos2(x_left, rect.top() + 8.0), vec2((116.0_f32 * 460.0 / 215.0).round(), 116.0));
        w::image_or_skeleton(ui, t, img, &it.header, t.corner_small());
        let owned = self.owned.contains(&it.app_id);
        let wished = self.is_wishlisted(ctx, it.app_id);
        let right_w = (rect.width() * 0.28).clamp(170.0, 290.0);
        let mid = Rect::from_min_max(pos2(img.right() + 14.0, rect.top() + 8.0), pos2((rect.right() - right_w - 8.0).max(img.right() + 120.0), rect.bottom() - 4.0));
        let opened = resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(mid).layout(Layout::top_down(Align::Min)));
        c.set_clip_rect(mid.intersect(ui.clip_rect()));
        c.spacing_mut().item_spacing.y = 3.0;
        c.add(egui::Label::new(w::text_semibold(&it.name, 16.0, t.text)).truncate().selectable(false));
        let mut open_tag = None;
        let names: Vec<(u32, String)> = it.tags.iter().filter_map(|tag| Some((*tag, self.tag_name(*tag)?))).collect();
        if !names.is_empty() || it.demo.is_some() {
            c.horizontal(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for (tag, name) in names.iter().take(4) {
                    if small_tag(ui, t, name).on_hover_text(t!("Browse games with this tag")).clicked() {
                        open_tag = Some(*tag);
                    }
                }
                if names.len() > 4 {
                    let more = small_tag(ui, t, &format!("+{}", names.len() - 4)).on_hover_text(t!("More tags"));
                    egui::Popup::from_toggle_button_response(&more).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).width(320.0).show(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                            for (tag, name) in names.iter().skip(4) {
                                if small_tag(ui, t, name).on_hover_text(t!("Browse games with this tag")).clicked() {
                                    open_tag = Some(*tag);
                                }
                            }
                        });
                    });
                }
                if it.demo.is_some() {
                    w::pill(ui, t!("Demo available"), t.accent, t.accent.gamma_multiply(0.15));
                }
            });
        }
        if let Some(tag) = open_tag {
            self.open_tag(tag);
        }
        c.horizontal(|ui| {
            let (win, mac, lin) = it.platforms;
            let systems: String = [(win, icons::WINDOWS), (mac, icons::APPLE), (lin, icons::LINUX)].iter().filter(|x| x.0).map(|x| x.1).collect::<Vec<_>>().join("  ");
            ui.label(w::text(systems, 13.5, t.text_faint));
            if it.coming_soon {
                ui.label(w::text(format!("{}  {}", icons::CALENDAR, i18n::tr_dyn(&it.release_text)), 12.5, t.accent));
            } else if it.released > 0 {
                ui.label(w::text(ui::date(it.released), 12.5, t.text_faint));
            }
            if owned {
                w::pill(ui, t!("In library"), t.play, t.play.gamma_multiply(0.15));
            }
        });
        if it.review_count > 0 {
            rating_bar(&mut c, t, it.review_percent, it.review_count, 130.0);
        }

        let side = Rect::from_min_max(pos2(rect.right() - right_w, rect.top() + 8.0), rect.max - vec2(10.0, 8.0));
        let mut r = ui.new_child(egui::UiBuilder::new().max_rect(side).layout(Layout::right_to_left(Align::Min)));
        r.set_clip_rect(side.intersect(ui.clip_rect()));
        let heart = if wished { t!("Remove from wishlist") } else { t!("Add to wishlist") };
        if w::heart_button(&mut r, t, wished, heart).clicked() {
            self.toggle_wishlist(ctx, it.app_id, wished);
        }
        if let Some(ends) = discount_ends_text(it.discount_ends) {
            let ends_rect = Rect::from_min_max(pos2(side.left(), side.bottom() - 48.0), pos2(side.right(), side.bottom() - 32.0));
            let mut e = ui.new_child(egui::UiBuilder::new().max_rect(ends_rect).layout(Layout::right_to_left(Align::Center)));
            e.set_clip_rect(ends_rect.intersect(ui.clip_rect()));
            e.add(egui::Label::new(w::text(ends, 11.5, t.text_dim)).truncate().selectable(false));
        }
        let mut b = ui.new_child(egui::UiBuilder::new().max_rect(side).layout(Layout::right_to_left(Align::Max)));
        b.set_clip_rect(side.intersect(ui.clip_rect()));
        if !owned && !it.coming_soon {
            if let Some(pkg) = it.package_id {
                let clicked = if side.width() < 215.0 {
                    w::icon_button(&mut b, t, icons::CART, t!("Add to cart"), false, 0).clicked()
                } else {
                    w::button(&mut b, t, Some(icons::CART), t!("Add to cart"), w::Kind::Primary, w::Size::Sm).clicked()
                };
                if clicked {
                    let price = it.formatted_price.clone().map(|p| (p, it.formatted_original.clone(), it.price.discount));
                    self.add_to_cart(ctx, it.app_id, pkg, &it.name, price, owned);
                }
            }
        }
        match &it.formatted_price {
            _ if it.coming_soon => {
                b.label(w::text_semibold(t!("Coming soon"), 14.0, t.text_dim));
            }
            Some(p) => {
                b.horizontal(|ui| {
                    w::price_tag(ui, t, p, it.formatted_original.as_deref(), it.price.discount);
                });
            }
            None if it.is_free => {
                b.label(w::text_semibold(t!("Free"), 14.0, t.text));
            }
            None => {}
        }
        opened
    }

    /// Drag handle + editable place number for reordering a wishlist row.
    #[allow(clippy::too_many_arguments)]
    fn rank_column(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: AppId, pos: usize, total: usize, col: Rect) {
        let grip = Rect::from_min_size(pos2(col.left(), col.center().y - 26.0), vec2(col.width(), 24.0));
        let g = ui.interact(grip, ui.scope_id().with(("wish-grip", id)), Sense::drag());
        let drag_color = if g.hovered() || self.wish.drag == Some(id) { t.text } else { t.text_faint };
        ui.painter().text(grip.center(), Align2::CENTER_CENTER, icons::LIST, w::font(15.0), drag_color);
        let g = g.on_hover_text(t!("Drag to change its place in your wishlist")).on_hover_cursor(egui::CursorIcon::Grab);
        if g.drag_started() {
            self.wish.drag = Some(id);
            self.wish.rank_edit = None;
        }
        let num = Rect::from_min_size(pos2(col.left() + 4.0, col.center().y + 2.0), vec2(col.width() - 8.0, 24.0));
        let editing = self.wish.rank_edit.as_ref().is_some_and(|(a, _, _)| *a == id);
        if editing {
            let mut move_to = None;
            let mut close = false;
            if let Some((_, text, fresh)) = &mut self.wish.rank_edit {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(num));
                let te = child.add(
                    egui::TextEdit::singleline(text)
                        .font(w::font_semibold(12.5))
                        .horizontal_align(Align::Center)
                        .desired_width(num.width()),
                );
                w::autofocus(&te, *fresh);
                *fresh = false;
                if te.ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                } else if te.lost_focus() && te.ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Ok(n) = text.trim().parse::<usize>() {
                        move_to = Some(n.saturating_sub(1).min(total.saturating_sub(1)));
                    }
                    close = true;
                } else if te.lost_focus() {
                    close = true;
                }
            }
            if close {
                self.wish.rank_edit = None;
            }
            if let Some(to) = move_to {
                self.wish_move(ctx, id, to);
            }
        } else {
            let nr = ui.interact(num, ui.scope_id().with(("wish-num", id)), Sense::click());
            let hover = ui.ctx().animate_bool_with_time(nr.id, nr.hovered(), 0.1);
            if hover > 0.0 {
                ui.painter().rect_filled(num, t.corner_small(), t.surface_hover.gamma_multiply(hover));
            }
            ui.painter().text(num.center(), Align2::CENTER_CENTER, format!("{}", pos + 1), w::font_semibold(13.0), if nr.hovered() { t.text } else { t.text_dim });
            if nr.on_hover_text(t!("Type a number to move it to that place")).on_hover_cursor(egui::CursorIcon::Text).clicked() {
                self.wish.rank_edit = Some((id, (pos + 1).to_string(), true));
            }
        }
    }

    // ---------- wishlist ----------

    pub(super) fn wishlist_page(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        w::page_title(ui, t, t!("Your wishlist"), None);
        let Some(list) = self.wishlist_entries(ctx) else {
            w::loading(ui, t, t!("Loading…"));
            return;
        };
        if list.is_empty() {
            w::empty(ui, t, icons::HEART, t!("Your wishlist is empty"), t!("Add games from their store pages and you'll be told when they go on sale."));
            return;
        }
        let mut sorted_ids: Vec<AppId> = list.iter().map(|x| x.0).collect();
        sorted_ids.sort_unstable();
        if self.wish_items.as_ref().is_none_or(|(k, _)| *k != sorted_ids) {
            let (api, ids2) = (self.api.clone(), sorted_ids.clone());
            let slot = self.fetch(async move { api.items(&ids2).await.map(std::sync::Arc::from) });
            self.wish_items = Some((sorted_ids, slot));
        }
        let Some(Ok(items)) = self.wish_items.as_ref().and_then(|(_, s)| snapshot(s)) else {
            w::loading(ui, t, t!("Loading…"));
            return;
        };

        // Order by pending drag result, else Steam's wishlist rank order.
        let mut order: Vec<AppId> = if let Some(pending) = &self.wish.pending {
            let mut v = pending.clone();
            for &(id, _) in &list {
                if !v.contains(&id) {
                    v.push(id);
                }
            }
            v.retain(|id| list.iter().any(|(a, _)| a == id));
            v
        } else {
            list.iter().map(|(id, _)| *id).collect()
        };
        if order.is_empty() {
            order = list.iter().map(|(id, _)| *id).collect();
        }
        let rank_of: std::collections::HashMap<AppId, usize> = order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let total_ranked = order.len();
        let on_sale = items.iter().filter(|i| i.price.discount > 0).count();

        let side_w = 240.0;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
            ui.label(w::text(t!("{n} games · {sale} on sale now", n = items.len(), sale = on_sale), 13.5, t.text_dim));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                egui::ComboBox::from_id_salt("wish-sort")
                    .selected_text(i18n::tr_dyn(self.wish.sort.label()))
                    .show_ui(ui, |ui| {
                        for s in WishSort::ALL {
                            ui.selectable_value(&mut self.wish.sort, s, i18n::tr_dyn(s.label()));
                        }
                    });
                ui.label(w::text(t!("Sort by"), 12.5, t.text_dim));
                ui.add_space(6.0);
                if w::icon_button(ui, t, icons::GRID, t!("Grid"), self.wish.grid, 0).clicked() {
                    self.wish.grid = true;
                    self.wish.drag = None;
                    self.wish.rank_edit = None;
                }
                if w::icon_button(ui, t, icons::LIST, t!("List"), !self.wish.grid, 0).clicked() {
                    self.wish.grid = false;
                }
                ui.label(w::text(t!("View|layout"), 12.5, t.text_dim));
            });
        });
        ui.add_space(10.0);

        let q = self.wish.query.trim().to_lowercase();
        let mut shown: Vec<_> = items
            .iter()
            .filter(|i| model::contains_ignore_case(&i.name, &q))
            .filter(|i| match self.wish.platform {
                1 => i.platforms.0,
                2 => i.platforms.1,
                3 => i.platforms.2,
                _ => true,
            })
            .filter(|i| !self.wish.on_sale || i.price.discount > 0)
            .filter(|i| match self.wish.max_price {
                Some(0) => i.is_free || i.price.final_ == Some(0),
                Some(max) => i.is_free || i.price.final_.is_some_and(|p| p <= max),
                None => true,
            })
            .collect();
        match self.wish.sort {
            WishSort::Rank => shown.sort_by_key(|i| rank_of.get(&i.app_id).copied().unwrap_or(usize::MAX)),
            WishSort::Discount => shown.sort_by(|a, b| (std::cmp::Reverse(a.price.discount)).cmp(&(std::cmp::Reverse(b.price.discount))).then_with(|| model::cmp_ignore_case(&a.name, &b.name))),
            WishSort::PriceLow => shown.sort_by(|a, b| (a.price.final_.unwrap_or(u64::MAX)).cmp(&(b.price.final_.unwrap_or(u64::MAX))).then_with(|| model::cmp_ignore_case(&a.name, &b.name))),
            WishSort::PriceHigh => shown.sort_by(|a, b| (std::cmp::Reverse(a.price.final_.unwrap_or(0))).cmp(&(std::cmp::Reverse(b.price.final_.unwrap_or(0)))).then_with(|| model::cmp_ignore_case(&a.name, &b.name))),
            WishSort::Name => shown.sort_by(|a, b| model::cmp_ignore_case(&a.name, &b.name)),
            WishSort::Released => shown.sort_by(|a, b| (std::cmp::Reverse(a.released)).cmp(&(std::cmp::Reverse(b.released))).then_with(|| model::cmp_ignore_case(&a.name, &b.name))),
            WishSort::Reviews => shown.sort_by_key(|i| (std::cmp::Reverse(i.review_percent), std::cmp::Reverse(i.review_count))),
        }

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 24.0;
            ui.vertical(|ui| {
                ui.set_width(side_w);
                let active = wishlist_filter_count(&self.wish);
                let v = &mut self.wish;
                let reset = filter_column(ui, t, active, |ui| {
                    let width = ui.available_width();
                    w::search(ui, t, &mut v.query, t!("Search by name"), width);
                    ui.add_space(10.0);
                    w::check(ui, t, &mut v.on_sale, t!("Special offers only"));
                    ui.add_space(10.0);
                    ui.label(w::caption_text(t, t!("PLATFORM")));
                    for (p, label) in [(0u8, "Any platform"), (1, "Windows"), (2, "macOS"), (3, "Linux")] {
                        if w::radio(ui, t, v.platform == p, i18n::tr_dyn(label)).clicked() {
                            v.platform = p;
                        }
                    }
                    ui.add_space(10.0);
                    ui.label(w::caption_text(t, t!("PRICE")));
                    for (max, label) in [(None, "Any price"), (Some(0), "Free"), (Some(500), "Under $5"), (Some(1000), "Under $10"), (Some(2000), "Under $20"), (Some(4000), "Under $40")] {
                        if w::radio(ui, t, v.max_price == max, i18n::tr_dyn(label)).clicked() {
                            v.max_price = max;
                        }
                    }
                });
                if reset {
                    v.query.clear();
                    v.platform = 0;
                    v.max_price = None;
                    v.on_sale = false;
                }
            });
            ui.vertical(|ui| {
                if shown.is_empty() {
                    w::empty(ui, t, icons::SEARCH, t!("No results"), t!("Try changing or clearing your filters."));
                    return;
                }
                self.wish.rows.clear();
                self.wish.reorderable = self.wish.sort == WishSort::Rank && !self.wish.grid;
                if self.wish.grid {
                    let gap = 16.0;
                    let per = (((ui.available_width() + gap) / (250.0 + gap)).floor() as usize).max(1);
                    let cw = (ui.available_width() - gap * (per as f32 - 1.0)) / per as f32;
                    for row in shown.chunks(per) {
                        if w::skip_offscreen_row(ui, cw * 215.0 / 460.0 + 64.0) {
                            ui.add_space(gap);
                            continue;
                        }
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            for &it in row {
                                let item = super::query_card(it.clone());
                                let owned = self.owned.contains(&it.app_id);
                                if self.store_card(ui, t, &item, cw, owned).clicked() {
                                    self.open(View::App(it.app_id));
                                }
                            }
                        });
                        ui.add_space(gap);
                    }
                } else {
                    for it in &shown {
                        let rank = if self.wish.reorderable { rank_of.get(&it.app_id).map(|&pos| (pos, total_ranked)) } else { None };
                        if self.result_row(ui, t, ctx, it, rank) {
                            self.open(View::App(it.app_id));
                        }
                        ui.add_space(6.0);
                    }
                    self.wish_drag(ui, t, ctx, &order, &shown);
                }
            });
        });
    }

    fn wish_drag(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, order: &[AppId], shown: &[&QueryItem]) {
        let Some(dragged) = self.wish.drag else { return };
        let Some(pos) = ui.ctx().pointer_latest_pos() else {
            if !ui.input(|i| i.pointer.primary_down()) {
                self.wish.drag = None;
            }
            return;
        };
        let rows = &self.wish.rows;
        let Some(&(first_id, first_rank, first_rect)) = rows.first() else {
            self.wish.drag = None;
            return;
        };
        let _ = first_id;
        // Pick insertion index and indicator Y from pointer Y vs row midpoints.
        let mut target = rows.last().map(|(_, r, _)| r + 1).unwrap_or(order.len());
        let mut line_y = rows.last().map(|(_, _, r)| r.bottom() + 3.0).unwrap_or(first_rect.top());
        for (i, (_, rank, rect)) in rows.iter().enumerate() {
            if pos.y < rect.center().y {
                target = *rank;
                line_y = if i == 0 { rect.top() - 3.0 } else { (rows[i - 1].2.bottom() + rect.top()) / 2.0 };
                break;
            }
        }
        let from_rank = rows.iter().find(|(id, _, _)| *id == dragged).map(|(_, r, _)| *r).unwrap_or(first_rank);
        let final_to = if target > from_rank { target.saturating_sub(1) } else { target };

        // Draw accent drop line across the row column.
        let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("wish-drop-line")));
        let x0 = first_rect.left();
        let x1 = first_rect.right();
        p.line_segment([pos2(x0, line_y), pos2(x1, line_y)], Stroke::new(2.0, t.accent));
        p.circle_filled(pos2(x0, line_y), 4.0, t.accent);
        p.circle_filled(pos2(x1, line_y), 4.0, t.accent);

        if let Some(it) = shown.iter().find(|i| i.app_id == dragged) {
            let label = format!("#{}  {}", final_to + 1, it.name);
            let galley = p.layout_no_wrap(label, w::font_semibold(12.5), t.on_accent);
            let pill = Rect::from_min_size(pos + vec2(14.0, 12.0), galley.size() + vec2(16.0, 8.0));
            p.rect_filled(pill, t.corner_small(), t.accent);
            p.galley(pill.min + vec2(8.0, 4.0), galley, t.on_accent);
        }

        // Scroll gently if dragging near the top or bottom of the viewport.
        let clip = ui.clip_rect();
        if pos.y < clip.top() + 48.0 {
            ui.scroll_with_delta(vec2(0.0, 10.0));
            ui.ctx().request_repaint();
        } else if pos.y > clip.bottom() - 48.0 {
            ui.scroll_with_delta(vec2(0.0, -10.0));
            ui.ctx().request_repaint();
        }

        if !ui.input(|i| i.pointer.primary_down()) {
            self.wish.drag = None;
            self.wish_move(ctx, dragged, final_to);
        }
    }

    fn wish_move(&mut self, ctx: &mut Ctx, app_id: AppId, to: usize) {
        let mut order: Vec<AppId> = if let Some(pending) = &self.wish.pending {
            pending.clone()
        } else if let Some(list) = self.wishlist_entries(ctx) {
            list.into_iter().map(|(id, _)| id).collect()
        } else {
            return;
        };
        let Some(from) = order.iter().position(|id| *id == app_id) else { return };
        let to = to.min(order.len().saturating_sub(1));
        if from == to {
            return;
        }
        order.remove(from);
        order.insert(to, app_id);
        self.wish.pending = Some(order);
        if ctx.state.account != "demo" {
            ctx.send(Command::WishlistMove { app_id, to });
        }
    }

    // ---------- cart ----------

    pub(super) fn cart_page(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        w::page_title(ui, t, t!("Your cart"), None);
        let Some(lines) = ctx.state.cart.clone() else {
            w::loading(ui, t, t!("Loading…"));
            return;
        };
        if lines.is_empty() {
            w::empty(ui, t, icons::CART, t!("Your cart is empty"), t!("Add games from the store; it's the same cart as on Steam's website."));
            return;
        }
        let pkgs: Vec<u32> = lines.iter().map(|l| l.package_id).collect();
        if self.cart_names.as_ref().is_none_or(|(k, _)| *k != pkgs) {
            let (api, p2) = (self.api.clone(), pkgs.clone());
            let slot = self.fetch(async move { api.packages(&p2).await });
            self.cart_names = Some((pkgs.clone(), slot));
        }
        let names = self.cart_names.as_ref().and_then(|(_, s)| snapshot(s)).and_then(|r| r.ok()).unwrap_or_default();
        let mut acts = Vec::new();
        for l in &lines {
            let (name, art, app) = names.iter().find(|n| n.0 == l.package_id).map(|n| (n.1.clone(), n.2.clone(), n.3)).unwrap_or_else(|| (String::new(), String::new(), 0));
            let owned = app != 0 && self.owned.contains(&app);
            let same: Vec<model::CartLine> = lines.iter().filter(|x| x.package_id == l.package_id).cloned().collect();
            if let Some(a) = cart_row(ui, t, ctx, l, &same, &name, &art, None, owned) {
                acts.push((a, app, l.package_id, name.clone()));
            }
            ui.add_space(6.0);
        }
        for (a, app, pkg, name) in acts {
            self.cart_act(ctx, a, app, pkg, &name);
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let r = w::button(ui, t, Some(icons::CART), t!("Continue to checkout"), w::Kind::Primary, w::Size::Lg)
                .on_hover_text(t!("Steam only takes payment on its website, so checkout opens in your browser."));
            if r.clicked() {
                crate::links::open_http("https://store.steampowered.com/cart");
            }
            ui.label(w::text(t!("Payment happens on Steam's checkout page. Your cart is already there."), 12.5, t.text_faint));
        });
    }

    // ---------- purchase box helpers ----------

    /// The buy box's buttons, like Steam's: a wishlist heart and one
    /// "Add to Cart" (or "In Cart") button. Gifts are chosen in the cart.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn purchase_actions(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: AppId, package: Option<u32>, name: &str, price: Option<(String, Option<String>, u8)>, owned: bool) {
        let wished = self.is_wishlisted(ctx, id);
        if !owned && w::heart_button(ui, t, wished, if wished { t!("On wishlist") } else { t!("Add to wishlist") }).clicked() {
            self.toggle_wishlist(ctx, id, wished);
        }
        let Some(pkg) = package else { return };
        let copies = ctx.state.cart.as_ref().map(|c| c.iter().filter(|l| l.package_id == pkg).count()).unwrap_or(0);
        ui.add_space(6.0);
        if copies > 0 {
            let label = if copies > 1 { t!("In cart ({n})", n = copies).to_string() } else { t!("In cart").to_string() };
            if w::button(ui, t, Some(icons::CART), &label, w::Kind::Play, w::Size::Lg).on_hover_text(t!("Add another copy, or make one a gift")).clicked() {
                self.cart_popup = Some(CartPopup { app: id, package: pkg, name: name.to_string(), price, owned, waiting: false });
            }
        } else if w::button(ui, t, Some(icons::CART), t!("Add to cart"), w::Kind::Primary, w::Size::Lg).clicked() {
            self.add_to_cart(ctx, id, pkg, name, price, owned);
        }
    }

    /// Add a copy like Steam does: for this account when it can be, else as
    /// a gift (already owned, or a copy for the account is in the cart).
    pub(super) fn add_to_cart(&mut self, ctx: &mut Ctx, app: AppId, package: u32, name: &str, price: Option<(String, Option<String>, u8)>, owned: bool) {
        let mine_in_cart = ctx.state.cart.as_ref().is_some_and(|c| c.iter().any(|l| l.package_id == package && !l.is_gift));
        ctx.send(Command::CartAdd { package_id: package, gift: owned || mine_in_cart, gift_to: None });
        self.cart_popup = Some(CartPopup { app, package, name: name.to_string(), price, owned, waiting: true });
    }

    /// Steam's "Added to your cart!" window: every copy of this game in the
    /// cart, each for the account or a gift, with Add and Remove.
    pub(super) fn cart_popup_ui(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let Some(pop) = self.cart_popup.clone() else { return };
        let all = ctx.state.cart.clone().unwrap_or_default();
        let lines: Vec<model::CartLine> = all.iter().filter(|l| l.package_id == pop.package).cloned().collect();
        // Every copy removed: nothing left to show.
        if lines.is_empty() && !pop.waiting {
            self.cart_popup = None;
            return;
        }
        if !lines.is_empty() && pop.waiting {
            if let Some(p) = &mut self.cart_popup {
                p.waiting = false;
            }
        }
        let mut close = false;
        let mut acts = Vec::new();
        ui::widgets::modal(ui.ctx(), egui::Id::unique("cart-popup")).show(ui.ctx(), |ui| {
            ui.set_width(620.0);
            ui.horizontal(|ui| {
                ui.label(w::text_bold(t!("Added to your cart!"), 20.0, t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::icon_button(ui, t, icons::X, t!("Close"), false, 0).clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(8.0);
            if lines.is_empty() {
                w::loading(ui, t, t!("Adding…"));
            }
            egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
                for l in &lines {
                    let art = model::art::header(pop.app);
                    if let Some(a) = cart_row(ui, t, ctx, l, &lines, &pop.name, &art, pop.price.as_ref(), pop.owned) {
                        acts.push(a);
                    }
                    ui.add_space(6.0);
                }
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if w::button(ui, t, None, t!("Continue shopping"), w::Kind::Secondary, w::Size::Md).clicked() {
                    close = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::button(ui, t, Some(icons::CART), &t!("View my cart ({n})", n = all.len()), w::Kind::Primary, w::Size::Md).clicked() {
                        close = true;
                        self.open(View::Cart);
                    }
                });
            });
        });
        for a in acts {
            self.cart_act(ctx, a, pop.app, pop.package, &pop.name);
        }
        if close {
            self.cart_popup = None;
        }
    }

    /// Carry out what a cart row asked for.
    pub(super) fn cart_act(&mut self, ctx: &mut Ctx, act: CartAct, app: AppId, package: u32, name: &str) {
        match act {
            CartAct::SetGift(line, gift) => ctx.send(Command::CartModify { line_item_id: line, gift, gift_to: None }),
            // Another copy: always a gift (one copy per account).
            CartAct::AddCopy => ctx.send(Command::CartAdd { package_id: package, gift: true, gift_to: None }),
            CartAct::Remove(line) => ctx.send(Command::CartRemove { line_item_id: line }),
            CartAct::ChooseFriend(line, to) => {
                self.gift = Some(Gift { app, package, name: name.to_string(), to, line: Some(line), search: String::new(), checks: Default::default() });
            }
        }
    }

    pub(super) fn gift_dialog(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        // Start the region check for the picked friend (once per friend).
        let pending = self.gift.as_ref().and_then(|g| g.to.filter(|to| !g.checks.contains_key(to)).map(|to| (g.app, to)));
        if let Some((app, to)) = pending {
            let (api, cc) = (self.api.clone(), self.locale.0.clone());
            let slot = self.fetch(async move { api.gift_check(app, &cc, to).await });
            if let Some(g) = &mut self.gift {
                g.checks.insert(to, slot);
            }
        }
        let Some(g) = &mut self.gift else { return };
        let mut close = false;
        let mut send = None;
        ui::widgets::modal(ui.ctx(), egui::Id::unique(("gift", g.package))).show(ui.ctx(), |ui| {
            ui.set_width(440.0);
            ui.label(w::text_bold(t!("Buy {name} as a gift", name = g.name), 19.0, t.text));
            ui.label(w::text(t!("Pick a friend. The gift goes into your cart and is delivered after checkout."), 13.0, t.text_dim));
            ui.add_space(8.0);
            w::search(ui, t, &mut g.search, &t!("Search friends"), ui.available_width());
            ui.add_space(6.0);
            // Online friends first, then by name; avatar and status on each row.
            let q = g.search.trim().to_lowercase();
            let mut friends: Vec<_> = ctx.state.friend_list().into_iter().filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q)).collect();
            friends.sort_by_key(|p| (p.state == model::PersonaState::Offline, !p.in_game()));
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for p in &friends {
                    let sel = g.to == Some(p.steam_id);
                    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
                    let hover = ui.ctx().animate_bool_with_time(r.id, r.hovered(), 0.1);
                    let fill = if sel { t.accent.gamma_multiply(0.22) } else { t.surface_hover.gamma_multiply(hover) };
                    ui.painter().rect_filled(rect, t.corner_small(), fill);
                    if sel {
                        ui.painter().rect_stroke(rect, t.corner_small(), Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
                    }
                    let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 5.0))).layout(Layout::left_to_right(Align::Center)));
                    w::avatar(&mut row, t, &model::art::avatar(p.avatar_hash.as_deref()), 34.0, Some(crate::shell::status_color(t, p)));
                    row.add_space(4.0);
                    row.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        // A friend Steam hasn't sent a name for yet shows their SteamID.
                        let unnamed = p.name.is_empty() || p.name.chars().all(|c| c.is_ascii_digit());
                        let name = if unnamed { t!("Friend {id}", id = p.steam_id) } else { p.name.clone() };
                        ui.add(egui::Label::new(w::text_medium(name, 14.0, if unnamed { t.text_dim } else { t.text })).truncate());
                        ui.add(egui::Label::new(w::text(crate::shell::status_text(ctx.state, p), 12.0, crate::shell::status_color(t, p))).truncate());
                    });
                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        g.to = Some(p.steam_id);
                    }
                }
                if friends.is_empty() {
                    ui.label(w::text(t!("No friends match."), 13.0, t.text_faint));
                }
            });
            ui.add_space(8.0);
            // Can Steam deliver it to them? (Regional price rule.)
            if let Some(to) = g.to {
                let name = ctx.state.persona(to).name;
                let line = match g.checks.get(&to).and_then(snapshot) {
                    None => (icons::CLOCK, t!("Checking whether Steam allows this gift in {name}'s region…", name = name), t.text_dim),
                    Some(Err(_)) => (icons::INFO, t!("Couldn't check {name}'s region. Steam decides at checkout.", name = name), t.text_dim),
                    Some(Ok(c)) => gift_line(t, &c, &name),
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label(w::text(line.0, 15.0, line.2));
                    ui.add(egui::Label::new(w::text(line.1, 13.0, line.2)).wrap());
                });
                ui.label(w::text(t!("Steam blocks gifts when the game is more than about 10% cheaper in the friend's region (base prices, converted)."), 11.5, t.text_faint));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let ok = if g.line.is_some() { t!("Give it to them") } else { t!("Add gift to cart") };
                if ui.add_enabled_ui(g.to.is_some(), |ui| w::button(ui, t, Some(icons::CART), ok, w::Kind::Primary, w::Size::Md)).inner.clicked() {
                    send = g.to.map(|to| (g.package, to));
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Secondary, w::Size::Md).clicked() {
                    close = true;
                }
            });
        });
        if let Some((package, to)) = send {
            match self.gift.as_ref().and_then(|g| g.line) {
                Some(line) => ctx.send(Command::CartModify { line_item_id: line, gift: true, gift_to: Some(to) }),
                None => ctx.send(Command::CartAdd { package_id: package, gift: true, gift_to: Some(to) }),
            }
            close = true;
        }
        if close {
            self.gift = None;
        }
    }

    // ---------- price history ----------

    /// Remember the prices we see, so there's a history even without IsThereAnyDeal.
    pub(super) fn log_price(&mut self, id: AppId, cents: u64) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let entry = self.price_log.entry(id).or_default();
        if entry.last().is_none_or(|(_, c)| *c != cents) {
            entry.push((now, cents));
            self.price_log_dirty = true;
        }
    }

    pub(super) fn price_history(&mut self, ui: &mut Ui, t: &Theme, id: AppId) {
        w::section(ui, t, t!("Price history"), None);
        w::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if self.itad_key.trim().is_empty() {
                let local = self.price_log.get(&id).cloned().unwrap_or_default();
                if local.len() >= 2 {
                    let pts: Vec<(u64, f64)> = local.iter().map(|(t, c)| (*t, *c as f64 / 100.0)).collect();
                    chart(ui, t, &pts, "");
                    ui.label(w::text(t!("Prices you've seen in Aurora."), 12.0, t.text_faint));
                } else {
                    ui.label(w::text(t!("Add a free IsThereAnyDeal API key in Settings → Store preferences to see this game's full price history and all-time low."), 13.0, t.text_dim));
                }
                return;
            }
            if !self.history.contains_key(&id) {
                let (api, key) = (self.api.clone(), self.itad_key.clone());
                let slot = self.fetch(async move { api.price_history(&key, id).await });
                self.history.insert(id, slot);
            }
            match snapshot(&self.history[&id]) {
                None => {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 160.0), Sense::hover());
                    w::skeleton(ui, t, r, t.corner_small());
                }
                Some(Err(e)) => {
                    ui.label(w::text(t!("Price history isn't available: {error}", error = e), 13.0, t.text_faint));
                }
                Some(Ok(h)) => {
                    if let Some((low, when)) = h.low {
                        ui.label(w::text_semibold(t!("All-time low on Steam: {price} ({date})", price = format!("{low:.2} {}", h.currency), date = ui::date(when)), 14.0, t.play));
                    }
                    let pts: Vec<(u64, f64)> = h.points.iter().map(|p| (p.time, p.price)).collect();
                    if pts.len() >= 2 {
                        chart(ui, t, &pts, &h.currency);
                    }
                    ui.horizontal(|ui| {
                        ui.label(w::text(t!("Price data:"), 12.0, t.text_faint));
                        if ui.link(w::text("IsThereAnyDeal", 12.0, t.accent)).clicked() {
                            crate::links::open_http(h.url.as_deref().unwrap_or("https://isthereanydeal.com"));
                        }
                    });
                }
            }
        });
        ui.add_space(16.0);
    }
}

/// A step chart of price over time.
fn chart(ui: &mut Ui, t: &Theme, pts: &[(u64, f64)], currency: &str) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 170.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, t.corner_small(), t.raised);
    let area = rect.shrink2(vec2(44.0, 16.0));
    let (t0, t1) = (pts.first().unwrap().0, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0).max(pts.last().unwrap().0 + 1));
    let max = pts.iter().map(|x| x.1).fold(0.0, f64::max).max(0.01) * 1.1;
    let x = |time: u64| area.left() + area.width() * ((time - t0) as f32 / (t1 - t0) as f32);
    let y = |v: f64| area.bottom() - area.height() * (v / max) as f32;
    for i in 0..=3 {
        let v = max * i as f64 / 3.0;
        p.hline(area.x_range(), y(v), Stroke::new(1.0, t.border));
        p.text(pos2(rect.left() + 6.0, y(v)), Align2::LEFT_CENTER, format!("{v:.0}"), w::font(10.5), t.text_faint);
    }
    let mut line: Vec<Pos2> = Vec::new();
    for (i, (time, v)) in pts.iter().enumerate() {
        let next_t = pts.get(i + 1).map(|n| n.0).unwrap_or(t1);
        line.push(pos2(x(*time), y(*v)));
        line.push(pos2(x(next_t), y(*v)));
    }
    p.add(egui::Shape::line(line, Stroke::new(2.0, t.accent)));
    if let Some(hover) = resp.hover_pos() {
        let time = t0 + ((hover.x - area.left()) / area.width() * (t1 - t0) as f32).max(0.0) as u64;
        if let Some((_, v)) = pts.iter().rev().find(|(pt, _)| *pt <= time) {
            p.vline(hover.x, area.y_range(), Stroke::new(1.0, t.text_faint));
            p.text(pos2(hover.x + 6.0, area.top()), Align2::LEFT_TOP, format!("{v:.2} {currency}  ·  {}", ui::date(time)), w::font_semibold(12.0), t.text);
        }
    }
    let _ = Color32::TRANSPARENT;
}

/// The region check, in words: (icon, text, color).
fn gift_line(t: &Theme, c: &store::gift::GiftCheck, name: &str) -> (&'static str, String, egui::Color32) {
    use store::gift::Verdict;
    let price = |p: &Option<(u64, String)>| p.as_ref().map(|(cents, cur)| store::Price::format(*cents, cur)).unwrap_or_default();
    match &c.verdict {
        Verdict::Ok => {
            let diff = c.difference.map(|d| format!("{d:+.0}%")).unwrap_or_default();
            (icons::CHECK, t!("{name} can receive it: {theirs} there vs {yours} for you ({diff}).", name = name, theirs = price(&c.theirs), yours = price(&c.yours), diff = diff), t.play)
        }
        Verdict::Blocked => {
            let diff = c.difference.map(|d| format!("{:.0}%", -d)).unwrap_or_default();
            (icons::WARNING, t!("Steam will most likely refuse: it's {diff} cheaper in {name}'s region ({theirs} vs {yours}). A Steam gift card works instead.", diff = diff, name = name, theirs = price(&c.theirs), yours = price(&c.yours)), t.danger)
        }
        Verdict::Unknown(why) if why == "no country" => (icons::INFO, t!("{name} hasn't set a country on their profile, so the region can't be checked.", name = name), t.text_dim),
        Verdict::Unknown(_) => (icons::INFO, t!("Can't compare prices for {name}'s region. Steam decides at checkout.", name = name), t.text_dim),
    }
}

/// The "Added to your cart!" popup for one game.
#[derive(Clone)]
pub struct CartPopup {
    pub app: AppId,
    pub package: u32,
    pub name: String,
    /// (price, original price, discount %) as shown on the game page.
    pub price: Option<(String, Option<String>, u8)>,
    pub owned: bool,
    /// Just added: Steam hasn't sent the cart back yet.
    pub waiting: bool,
}

/// What a cart row's controls asked for.
pub enum CartAct {
    /// "For my account" (false) or "This is a gift" (true).
    SetGift(u64, bool),
    /// Another copy of the same game.
    AddCopy,
    Remove(u64),
    /// Pick (or change) the friend a gift line is for.
    ChooseFriend(u64, Option<SteamId>),
}

/// One cart line like Steam's: art, name, "For my account" / "This is a
/// gift", price, and Add | Remove. `same` = every line of this package.
#[allow(clippy::too_many_arguments)]
fn cart_row(ui: &mut Ui, t: &Theme, ctx: &Ctx, l: &model::CartLine, same: &[model::CartLine], name: &str, art: &str, price: Option<&(String, Option<String>, u8)>, owned: bool) -> Option<CartAct> {
    let mut act = None;
    // One copy per account: "For my account" only while no other copy is
    // for the account, and not for a game already owned.
    let mine_elsewhere = same.iter().any(|x| x.line_item_id != l.line_item_id && !x.is_gift);
    let can_be_mine = !owned && !mine_elsewhere;
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(170.0, 80.0), Sense::hover());
            w::image_or_skeleton(ui, t, r, art, t.corner_small());
            ui.add_space(6.0);
            ui.vertical(|ui| {
                if name.is_empty() {
                    let (r, _) = ui.allocate_exact_size(vec2(220.0, 20.0), Sense::hover());
                    w::skeleton(ui, t, r.shrink2(vec2(0.0, 3.0)), t.corner_small());
                } else {
                    ui.add(egui::Label::new(w::text_semibold(name, 15.0, t.text)).truncate());
                }
                ui.add_space(4.0);
                let current = if l.is_gift { t!("This is a gift") } else { t!("For my account") };
                egui::ComboBox::from_id_salt(("cart-line", l.line_item_id)).selected_text(current).width(170.0).show_ui(ui, |ui| {
                    let mine = ui.add_enabled(can_be_mine || !l.is_gift, egui::Button::selectable(!l.is_gift, t!("For my account")));
                    let mine = if owned { mine.on_disabled_hover_text(t!("You already own this game")) } else { mine.on_disabled_hover_text(t!("Another copy is already for your account")) };
                    if mine.clicked() && l.is_gift {
                        act = Some(CartAct::SetGift(l.line_item_id, false));
                    }
                    if ui.add(egui::Button::selectable(l.is_gift, t!("This is a gift"))).clicked() && !l.is_gift {
                        act = Some(CartAct::SetGift(l.line_item_id, true));
                    }
                });
                if l.is_gift {
                    let base = ctx.state.steam_id & 0xFFFF_FFFF_0000_0000;
                    let to = l.gift_to.map(|acc| base | acc as u64);
                    ui.horizontal(|ui| {
                        match to {
                            Some(id) => {
                                ui.label(w::text(format!("{}  {}", icons::USER, t!("For {name}", name = ctx.state.persona(id).name)), 12.5, t.accent));
                                if ui.link(w::text(t!("Change"), 12.5, t.text_dim)).clicked() {
                                    act = Some(CartAct::ChooseFriend(l.line_item_id, to));
                                }
                            }
                            None => {
                                if ui.link(w::text(format!("{}  {}", icons::USER, t!("Choose a friend…")), 12.5, t.accent)).on_hover_text(t!("Or pick who gets it on Steam's checkout page")).clicked() {
                                    act = Some(CartAct::ChooseFriend(l.line_item_id, None));
                                }
                            }
                        }
                    });
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                ui.vertical(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| match price {
                        Some((label, original, discount)) => w::price_tag(ui, t, label, original.as_deref(), *discount),
                        None => {
                            ui.label(w::text_semibold(&l.price, 15.0, t.text));
                        }
                    });
                    ui.add_space(10.0);
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        if ui.link(w::text(t!("Remove"), 12.5, t.text_dim)).clicked() {
                            act = Some(CartAct::Remove(l.line_item_id));
                        }
                        ui.label(w::text("|", 12.5, t.text_faint));
                        if ui.link(w::text(t!("Add"), 12.5, t.text_dim)).on_hover_text(t!("Another copy, as a gift")).clicked() {
                            act = Some(CartAct::AddCopy);
                        }
                    });
                });
            });
        });
    });
    act
}

/// Filter card wrapper with "FILTERS" heading and a "Clear (n)" button.
fn filter_column(ui: &mut Ui, t: &Theme, active: usize, add: impl FnOnce(&mut Ui)) -> bool {
    let mut reset = false;
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(w::caption_text(t, &t!("Filters").to_uppercase()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if active > 0 { t!("Clear ({n})", n = active) } else { t!("Clear").to_string() };
                let r = ui
                    .add_enabled_ui(active > 0, |ui| w::button(ui, t, Some(icons::X), &label, w::Kind::Ghost, w::Size::Sm))
                    .inner
                    .on_hover_text(t!("Turn every filter off"))
                    .on_disabled_hover_text(t!("No filters are on"));
                reset = r.clicked();
            });
        });
        ui.add_space(8.0);
        add(ui);
    });
    reset
}

fn browse_filter_count(f: &store::query::Filters, hide_owned: bool) -> usize {
    f.tags.len()
        + usize::from(f.max_price.is_some() || f.only_free)
        + usize::from(f.only_discounted)
        + usize::from(f.coming_soon)
        + usize::from(f.min_review_score.is_some())
        + [(f.games, true), (f.dlc, false), (f.software, false), (f.demos, false), (f.soundtracks, false)]
            .iter()
            .filter(|(on, d)| on != d)
            .count()
        + usize::from(hide_owned)
}

fn wishlist_filter_count(v: &WishView) -> usize {
    usize::from(v.platform != 0) + usize::from(v.max_price.is_some()) + usize::from(v.on_sale) + usize::from(!v.query.trim().is_empty())
}

/// Compact clickable tag pill for Browse / Wishlist rows.
fn small_tag(ui: &mut Ui, t: &Theme, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), w::font_semibold(11.0), t.text_dim);
    let (r, resp) = ui.allocate_exact_size(galley.size() + vec2(14.0, 6.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    ui.painter().rect_filled(r, egui::CornerRadius::same(4), w::mix(t.raised, t.accent.gamma_multiply(0.35), hover));
    let color = w::mix(t.text_dim, t.text, hover);
    ui.painter().galley(r.center() - galley.size() / 2.0, galley, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Compact positive/negative split bar + review total for list rows.
fn rating_bar(ui: &mut Ui, t: &Theme, positive: u32, count: u64, width: f32) {
    let positive = positive.min(100);
    let color = if positive >= 70 { t.accent } else if positive >= 40 { t.away } else { t.danger };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let bar_w = width.clamp(56.0, 88.0);
        let (rect, _) = ui.allocate_exact_size(vec2(bar_w, 6.0), Sense::hover());
        if ui.is_rect_visible(rect) {
            let p = ui.painter();
            let r = egui::CornerRadius::same(3);
            p.rect_filled(rect, r, t.raised);
            if positive > 0 {
                let fill_w = (rect.width() * positive as f32 / 100.0).max(4.0);
                p.rect_filled(Rect::from_min_size(rect.min, vec2(fill_w, rect.height())), r, color);
            }
        }
        ui.add(
            egui::Label::new(w::text(
                format!("{positive}% · {}", t!("{count} user reviews", count = super::thousands(count))),
                11.5,
                t.text_dim,
            ))
            .truncate()
            .selectable(false),
        );
    });
}

/// Text telling when a sale ends (hours under 48h, short date beyond).
pub(super) fn discount_ends_text(ends: u64) -> Option<String> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    discount_ends_at(ends, now)
}

fn discount_ends_at(ends: u64, now: u64) -> Option<String> {
    if ends <= now {
        return None;
    }
    let left = ends - now;
    if left < 48 * 3600 {
        let hours = left.div_ceil(3600).max(1);
        Some(t!("Ends in {n} hours", n = hours))
    } else {
        Some(t!("Offer ends {date}", date = ui::date_time(ends)))
    }
}

#[cfg(test)]
mod tests {
    use super::{WishView, browse_filter_count, discount_ends_at, wishlist_filter_count};

    #[test]
    fn counts_browse_and_wishlist_filters_accurately() {
        let mut f = store::query::Filters::default();
        assert_eq!(browse_filter_count(&f, false), 0);
        f.only_discounted = true;
        f.coming_soon = true;
        f.tags.push(19);
        assert_eq!(browse_filter_count(&f, true), 4);

        let mut w = WishView::default();
        assert_eq!(wishlist_filter_count(&w), 0);
        w.on_sale = true;
        w.platform = 3;
        w.max_price = Some(500);
        w.query = "hades".into();
        assert_eq!(wishlist_filter_count(&w), 4);
    }

    #[test]
    fn formats_discount_expiry_windows() {
        let now = 1_700_000_000;
        assert_eq!(discount_ends_at(0, now), None);
        assert_eq!(discount_ends_at(now, now), None);
        assert!(discount_ends_at(now + 7_200, now).is_some_and(|s| s.contains('2')));
        assert!(discount_ends_at(now + 86_400 * 5, now).is_some_and(|s| s.contains("2023")));
    }
}
