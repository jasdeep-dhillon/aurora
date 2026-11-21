//! A game's reviews in a window, shared by the library and the store: what
//! players say (Steam's review feed with polarity, sort, time-window and
//! language filters), and your own review, which you can write, edit or delete.
//! Filter controls adapted from upstream work by `madonchik123` (PR #74 `be8d333`).

use client::Command;
use egui::{Align, Layout, Margin, Ui, vec2};
use extensions::Ctx;
use i18n::t;
use model::{AppId, ReviewFilter, ReviewKind, ReviewOrder, ReviewWindow as TimeWindow};
use ui::{Theme, icons, widgets as w};

/// Steam's limits for a review's text.
const MAX_LEN: usize = 8000;

pub struct ReviewWindow {
    pub app: AppId,
    name: String,
    /// The game is on the account (Steam only takes reviews of owned games).
    owned: bool,
    filter: ReviewFilter,
    /// The editor is open (always, when you haven't written one yet).
    editing: bool,
    text: String,
    recommended: Option<bool>,
    public: bool,
    allow_comments: bool,
    saving: bool,
    confirm_delete: bool,
    /// Long reviews unfolded.
    expanded: std::collections::BTreeSet<String>,
    loaded: bool,
}

impl ReviewWindow {
    /// `write`: open with the editor ready (the store's "Write a review").
    pub fn new(app: AppId, name: &str, owned: bool, write: bool) -> Self {
        Self::with_filter(app, name, owned, write, ReviewFilter::default())
    }

    /// Open with a specific review filter selected (used by Store game page chips).
    pub fn with_filter(app: AppId, name: &str, owned: bool, write: bool, filter: ReviewFilter) -> Self {
        Self {
            app,
            name: name.to_string(),
            owned,
            filter,
            editing: write,
            text: String::new(),
            recommended: None,
            public: true,
            allow_comments: true,
            saving: false,
            confirm_delete: false,
            expanded: Default::default(),
            loaded: false,
        }
    }

    fn language(&self) -> String {
        i18n::language().steam_name().to_string()
    }

    fn load(&mut self, ctx: &mut Ctx) {
        if ctx.state.account != "demo" {
            ctx.send(Command::LoadAppReviews { app_id: self.app, language: self.language(), cursor: None, filter: self.filter });
        }
        if self.owned {
            ctx.send(Command::LoadMyReview { app_id: self.app });
        }
        self.loaded = true;
    }

    /// Look up cached reviews for `self.filter`; in `--demo` mode, filter and sort
    /// the seeded baseline reviews locally so every filter stays responsive offline.
    fn visible_reviews(&self, ctx: &Ctx) -> Option<Result<model::AppReviews, String>> {
        if let Some(res) = ctx.state.app_reviews.get(&(self.app, self.filter)) {
            return Some(res.clone());
        }
        if ctx.state.account != "demo" {
            return None;
        }
        let base = ctx.state.app_reviews.get(&(self.app, ReviewFilter::default()))?;
        Some(base.clone().map(|mut r| {
            r.items = filter_demo_reviews(&r.items, self.filter);
            r
        }))
    }

    /// Results of saving or deleting. Returns nothing; toasts say how it went.
    pub fn on_event(&mut self, event: &client::Event, ctx: &mut Ctx) {
        if let client::Event::ReviewSaved { app_id, deleted, result } = event {
            if *app_id != self.app {
                return;
            }
            self.saving = false;
            match result {
                Ok(()) => {
                    self.editing = false;
                    ctx.toast(if *deleted { t!("Review deleted") } else { t!("Review posted") });
                }
                Err(e) => ctx.toast(t!("Couldn't save the review: {error}", error = i18n::tr_dyn(e))),
            }
        }
    }

    /// Draw the window; returns false once it's closed.
    pub fn show(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) -> bool {
        if !self.loaded {
            self.load(ctx);
        }
        let mut open = true;
        // A modal like the app's other dialogs, sized to the window every frame.
        let screen = ui.ctx().content_rect();
        // Big: most of the window, so long reviews are easy to read.
        let width = (screen.width() - 120.0).clamp(360.0, 920.0);
        let height = (screen.height() - 240.0).max(280.0);
        w::modal(ui.ctx(), egui::Id::unique(("reviews", self.app))).show(ui.ctx(), |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.label(w::text_bold(t!("Reviews: {name}", name = self.name.clone()), 18.0, t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::icon_button(ui, t, icons::X, t!("Close"), false, 0).clicked() {
                        open = false;
                    }
                });
            });
            ui.add_space(6.0);
            self.summary(ui, t, ctx);
            ui.add_space(10.0);
            egui::ScrollArea::vertical().min_scrolled_height(height).max_height(height).auto_shrink([false, false]).show(ui, |ui| {
                self.mine(ui, t, ctx);
                ui.add_space(14.0);
                self.list(ui, t, ctx);
            });
        });
        if !self.confirm_delete && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        self.delete_dialog(ui, t, ctx);
        open
    }

    fn summary(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let reviews = self.visible_reviews(ctx).and_then(|r| r.ok());
        let before = self.filter;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            w::segmented(ui, t, &mut self.filter.kind, &[(ReviewKind::All, t!("All reviews")), (ReviewKind::Positive, t!("Positive|review")), (ReviewKind::Negative, t!("Negative|review"))]);
            w::segmented(ui, t, &mut self.filter.all_languages, &[(false, t!("Your language")), (true, t!("All languages"))]);
            egui::ComboBox::from_id_salt(("reviews-order", self.app))
                .selected_text(order_label(self.filter.order))
                .show_ui(ui, |ui| {
                    for o in [ReviewOrder::Helpful, ReviewOrder::Recent, ReviewOrder::Funny] {
                        ui.selectable_value(&mut self.filter.order, o, order_label(o));
                    }
                });
            if self.filter.order == ReviewOrder::Helpful {
                egui::ComboBox::from_id_salt(("reviews-window", self.app))
                    .selected_text(window_label(self.filter.window))
                    .show_ui(ui, |ui| {
                        for win in [
                            TimeWindow::AllTime,
                            TimeWindow::Day,
                            TimeWindow::Week,
                            TimeWindow::Month,
                            TimeWindow::ThreeMonths,
                            TimeWindow::SixMonths,
                            TimeWindow::Year,
                        ] {
                            ui.selectable_value(&mut self.filter.window, win, window_label(win));
                        }
                    });
            }
        });
        if before != self.filter && ctx.state.account != "demo" {
            ctx.send(Command::LoadAppReviews { app_id: self.app, language: self.language(), cursor: None, filter: self.filter });
        }
        if let Some(r) = reviews.filter(|r| r.total > 0) {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let pct = r.positive * 100 / (r.positive + r.negative).max(1);
                let color = if pct >= 70 { t.play } else if pct >= 40 { t.away } else { t.danger };
                ui.label(w::text_semibold(capitalized(&i18n::tr_dyn(&r.label)), 15.0, color));
                ui.label(w::text(t!("{pct}% of {n} reviews are positive", pct = pct, n = r.total), 13.0, t.text_dim));
            });
        }
    }

    /// Your review, or the editor.
    fn mine(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        ui.label(w::caption_text(t, t!("YOUR REVIEW")));
        ui.add_space(4.0);
        if !self.owned {
            ui.label(w::text(t!("You can review games that are in your library."), 13.0, t.text_faint));
            return;
        }
        let mine = ctx.state.my_reviews.get(&self.app).cloned();
        let existing = match &mine {
            None => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0).color(t.accent));
                    ui.label(w::text(t!("Looking for your review…"), 12.5, t.text_faint));
                });
                return;
            }
            Some(Err(e)) => {
                ui.label(w::text(t!("Couldn't check for your review: {error}", error = i18n::tr_dyn(e)), 12.5, t.danger));
                None
            }
            Some(Ok(r)) => r.clone(),
        };
        if let (Some(r), false) = (&existing, self.editing) {
            w::panel(t).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (icon, label, color) = if r.recommended { (icons::THUMBS_UP, t!("Recommends it"), t.play) } else { (icons::THUMBS_DOWN, t!("Doesn't recommend it"), t.danger) };
                    ui.label(w::text(format!("{icon}  {label}"), 14.0, color));
                    ui.label(w::text(&r.posted, 12.0, t.text_faint));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, t, Some(icons::TRASH), t!("Delete review"), w::Kind::Ghost, w::Size::Sm).clicked() {
                            self.confirm_delete = true;
                        }
                        if w::button(ui, t, Some(icons::NOTE), t!("Edit review"), w::Kind::Secondary, w::Size::Sm).clicked() {
                            self.editing = true;
                            self.text = r.text.clone();
                            self.recommended = Some(r.recommended);
                        }
                    });
                });
                ui.add_space(4.0);
                ui.add(egui::Label::new(w::text(&r.text, 13.5, t.text)).wrap());
            });
            return;
        }
        if existing.is_none() && !self.editing {
            ui.horizontal(|ui| {
                ui.label(w::text(t!("You haven't reviewed this game yet."), 13.0, t.text_dim));
                if w::button(ui, t, Some(icons::NOTE), t!("Write a review"), w::Kind::Primary, w::Size::Sm).clicked() {
                    self.editing = true;
                }
            });
            return;
        }
        self.editor(ui, t, ctx, existing.is_some());
    }

    fn editor(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, editing_existing: bool) {
        w::panel(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(w::text_semibold(t!("Do you recommend {name}?", name = self.name.clone()), 14.0, t.text));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for (value, icon, label) in [(true, icons::THUMBS_UP, t!("Yes")), (false, icons::THUMBS_DOWN, t!("No"))] {
                    let on = self.recommended == Some(value);
                    let kind = if on { w::Kind::Primary } else { w::Kind::Secondary };
                    if w::button(ui, t, Some(icon), label, kind, w::Size::Md).clicked() {
                        self.recommended = Some(value);
                    }
                }
            });
            ui.add_space(8.0);
            ui.add(
                egui::TextEdit::multiline(&mut self.text)
                    .hint_text(t!("What did you like or dislike? What should other players know?"))
                    .desired_rows(7)
                    .desired_width(f32::INFINITY)
                    .char_limit(MAX_LEN)
                    .margin(vec2(10.0, 8.0)),
            );
            ui.horizontal(|ui| {
                w::check(ui, t, &mut self.public, t!("Public"));
                w::check(ui, t, &mut self.allow_comments, t!("Allow comments"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(w::text(format!("{} / {MAX_LEN}", self.text.chars().count()), 11.5, t.text_faint));
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let ready = self.recommended.is_some() && !self.text.trim().is_empty() && !self.saving;
                let label = if self.saving { t!("Posting…") } else if editing_existing { t!("Update review") } else { t!("Post review") };
                if ui.add_enabled_ui(ready, |ui| w::button(ui, t, Some(icons::SEND), label, w::Kind::Primary, w::Size::Md)).inner.clicked() {
                    self.saving = true;
                    ctx.send(Command::PostReview {
                        app_id: self.app,
                        text: self.text.trim().to_string(),
                        recommended: self.recommended.unwrap_or(true),
                        public: self.public,
                        allow_comments: self.allow_comments,
                        language: i18n::language().steam_name().to_string(),
                    });
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Md).clicked() {
                    self.editing = false;
                }
            });
        });
    }

    /// What players say, filtered and sorted according to `self.filter`.
    fn list(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        ui.label(w::caption_text(t, t!("WHAT PLAYERS SAY")));
        ui.add_space(4.0);
        let Some(result) = self.visible_reviews(ctx) else {
            w::loading(ui, t, t!("Loading reviews…"));
            return;
        };
        let reviews = match result {
            Ok(r) => r,
            Err(e) => {
                ui.label(w::text(i18n::tr_dyn(&e), 13.0, t.danger));
                return;
            }
        };
        if reviews.items.is_empty() {
            let msg = if self.filter == ReviewFilter::default() { t!("No reviews in this language yet.") } else { t!("No reviews match these filters.") };
            ui.label(w::text(msg, 13.0, t.text_faint));
            return;
        }
        for r in &reviews.items {
            w::card(t).inner_margin(Margin::symmetric(18, 14)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (icon, label, color) = if r.recommended { (icons::THUMBS_UP, t!("Recommends it"), t.play) } else { (icons::THUMBS_DOWN, t!("Doesn't recommend it"), t.danger) };
                    ui.label(w::text(format!("{icon}  {label}"), 13.5, color));
                    let hours = format!("{:.1}", r.minutes_played as f64 / 60.0);
                    ui.label(w::text(t!("{hours} hrs on record", hours = hours), 12.0, t.text_dim));
                    if r.early_access {
                        ui.label(w::text(t!("Early Access review"), 11.5, t.away));
                    }
                    if r.free {
                        ui.label(w::text(t!("Got it free"), 11.5, t.text_faint));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(w::text(ui::ago(r.created), 11.5, t.text_faint));
                    });
                });
                let long = r.text.chars().count() > 600;
                let open = self.expanded.contains(&r.id);
                let text = if long && !open { format!("{}…", r.text.chars().take(600).collect::<String>()) } else { r.text.clone() };
                ui.add_space(4.0);
                ui.add(egui::Label::new(w::text(text, 14.5, t.text)).wrap());
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    if long && w::button(ui, t, None, if open { t!("Show less") } else { t!("Read more") }, w::Kind::Ghost, w::Size::Sm).clicked() {
                        if open {
                            self.expanded.remove(&r.id);
                        } else {
                            self.expanded.insert(r.id.clone());
                        }
                    }
                    if r.helpful > 0 {
                        ui.label(w::text(t!("{n} people found this helpful", n = r.helpful), 11.5, t.text_faint));
                    }
                });
            });
            ui.add_space(6.0);
        }
        if let Some(cursor) = reviews.cursor.clone().filter(|_| reviews.items.len() < reviews.total as usize) {
            ui.vertical_centered(|ui| {
                if w::button(ui, t, Some(icons::CARET_DOWN), t!("More reviews"), w::Kind::Secondary, w::Size::Md).clicked() {
                    ctx.send(Command::LoadAppReviews { app_id: self.app, language: self.language(), cursor: Some(cursor), filter: self.filter });
                }
            });
        }
    }

    fn delete_dialog(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        if !self.confirm_delete {
            return;
        }
        let mut close = false;
        w::modal(ui.ctx(), egui::Id::unique(("review-delete", self.app))).show(ui.ctx(), |ui| {
            ui.set_width(400.0);
            ui.label(w::text(icons::TRASH, 28.0, t.danger));
            ui.label(w::text_bold(t!("Delete your review?"), 18.0, t.text));
            ui.add(egui::Label::new(w::text(t!("Its votes and comments are deleted with it."), 13.0, t.text_dim)).wrap());
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if w::button(ui, t, Some(icons::TRASH), t!("Delete review"), w::Kind::Danger, w::Size::Md).clicked() {
                    self.saving = true;
                    ctx.send(Command::DeleteReview { app_id: self.app });
                    close = true;
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Secondary, w::Size::Md).clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.confirm_delete = false;
        }
    }
}

/// Steam's score labels come in the reader's language, sometimes lower
/// case ("très positives"): start them with a capital.
fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn order_label(order: ReviewOrder) -> &'static str {
    match order {
        ReviewOrder::Helpful => t!("Most helpful"),
        ReviewOrder::Recent => t!("Most recent"),
        ReviewOrder::Funny => t!("Funniest"),
    }
}

fn window_label(window: TimeWindow) -> &'static str {
    match window {
        TimeWindow::AllTime => t!("All time"),
        TimeWindow::Day => t!("Today"),
        TimeWindow::Week => t!("This week"),
        TimeWindow::Month => t!("This month"),
        TimeWindow::ThreeMonths => t!("Past 3 months"),
        TimeWindow::SixMonths => t!("Past 6 months"),
        TimeWindow::Year => t!("This year"),
    }
}

fn filter_demo_reviews(items: &[model::AppReview], filter: ReviewFilter) -> Vec<model::AppReview> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let cutoff = match (filter.order, filter.window) {
        (ReviewOrder::Helpful, TimeWindow::Day) => now.saturating_sub(86_400),
        (ReviewOrder::Helpful, TimeWindow::Week) => now.saturating_sub(86_400 * 7),
        (ReviewOrder::Helpful, TimeWindow::Month) => now.saturating_sub(86_400 * 30),
        (ReviewOrder::Helpful, TimeWindow::ThreeMonths) => now.saturating_sub(86_400 * 90),
        (ReviewOrder::Helpful, TimeWindow::SixMonths) => now.saturating_sub(86_400 * 180),
        (ReviewOrder::Helpful, TimeWindow::Year) => now.saturating_sub(86_400 * 365),
        _ => 0,
    };
    let mut out: Vec<model::AppReview> = items
        .iter()
        .filter(|r| match filter.kind {
            ReviewKind::All => true,
            ReviewKind::Positive => r.recommended,
            ReviewKind::Negative => !r.recommended,
        })
        .filter(|r| r.created >= cutoff)
        .cloned()
        .collect();
    match filter.order {
        ReviewOrder::Helpful => out.sort_by_key(|r| std::cmp::Reverse(r.helpful)),
        ReviewOrder::Recent => out.sort_by_key(|r| std::cmp::Reverse(r.created)),
        ReviewOrder::Funny => out.sort_by_key(|r| std::cmp::Reverse(r.funny)),
    }
    out
}
