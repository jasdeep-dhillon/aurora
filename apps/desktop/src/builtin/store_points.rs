//! Store: the Points Shop (ported and adapted from upstream `madonchik123/SteamInRust`, PR #101).
//!
//! Browses Steam's catalog of profile items, avatars, frames, backgrounds,
//! stickers, chat effects, keyboards, and startup movies via `ILoyaltyRewardsService`.
//! Clicking an item opens Steam's Points Shop page in the browser for redeeming.

use super::{Load, Shared, Store, View};
use egui::{Align, Align2, CornerRadius, Layout, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use extensions::Ctx;
use i18n::t;
use model::AppId;
use store::points::{RewardItem, RewardPage, class};
use ui::{Theme, icons, widgets as w};

const PAGE: u32 = 24;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub(super) enum Shelf {
    #[default]
    Featured,
    YourGames,
    Game(AppId),
    Kind(u32),
}

fn groups() -> Vec<(&'static str, Vec<(Shelf, &'static str)>)> {
    vec![
        (t!("Featured"), vec![(Shelf::Featured, t!("All items")), (Shelf::YourGames, t!("From your games"))]),
        (
            t!("Profile"),
            vec![
                (Shelf::Kind(class::AVATAR), t!("Animated avatars")),
                (Shelf::Kind(class::AVATAR_FRAME), t!("Avatar frames")),
                (Shelf::Kind(class::BACKGROUND), t!("Backgrounds")),
                (Shelf::Kind(class::MINI_BACKGROUND), t!("Mini-profile backgrounds")),
                (Shelf::Kind(class::PROFILE), t!("Game profiles")),
            ],
        ),
        (
            t!("Chat"),
            vec![
                (Shelf::Kind(class::STICKER), t!("Animated stickers")),
                (Shelf::Kind(class::CHAT_EFFECT), t!("Chat effects")),
                (Shelf::Kind(class::EMOTICON), t!("Emoticons")),
            ],
        ),
        (
            t!("Interface"),
            vec![(Shelf::Kind(class::KEYBOARD), t!("Keyboards")), (Shelf::Kind(class::STARTUP_MOVIE), t!("Startup movies"))],
        ),
    ]
}

fn kind_name(c: u32) -> &'static str {
    match c {
        class::AVATAR => t!("Animated avatar"),
        class::AVATAR_FRAME => t!("Avatar frame"),
        class::BACKGROUND => t!("Profile background"),
        class::MINI_BACKGROUND => t!("Mini-profile background"),
        class::PROFILE => t!("Game profile"),
        class::STICKER => t!("Animated sticker"),
        class::CHAT_EFFECT => t!("Chat effect"),
        class::EMOTICON => t!("Emoticon"),
        class::KEYBOARD => t!("Keyboard"),
        class::STARTUP_MOVIE => t!("Startup movie"),
        _ => t!("Item"),
    }
}

fn fmt_points(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[derive(Default)]
pub(super) struct Points {
    pub shelf: Shelf,
    pages: Vec<Shared<RewardPage>>,
    key: Option<(Shelf, Vec<AppId>)>,
    balance_asked: bool,
}

impl Store {
    pub(super) fn open_points_game(&mut self, id: AppId) {
        self.points.shelf = Shelf::Game(id);
        self.points.key = None;
        self.open(View::Points);
    }

    fn points_more(&mut self, cursor: Option<String>) {
        let Some((shelf, apps)) = self.points.key.clone() else { return };
        let classes: Vec<u32> = match shelf {
            Shelf::Kind(c) => vec![c],
            _ => Vec::new(),
        };
        let api = self.api.clone();
        let slot = self.fetch(async move { api.reward_items(&classes, &apps, cursor.as_deref(), PAGE).await });
        self.points.pages.push(slot);
    }

    pub(super) fn points_page(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let mut apps: Vec<AppId> = match self.points.shelf {
            Shelf::Game(id) => vec![id],
            Shelf::YourGames => {
                let mut v: Vec<AppId> = self.owned.iter().copied().collect();
                v.sort_unstable();
                v.truncate(50);
                v
            }
            _ => Vec::new(),
        };
        if self.points.shelf == Shelf::YourGames && apps.is_empty() && ctx.state.steam_id != 0 {
            apps = ctx.state.apps.as_ref().map(|m| m.keys().copied().take(50).collect()).unwrap_or_default();
        }
        let want_key = (self.points.shelf, apps);
        if self.points.key.as_ref() != Some(&want_key) {
            self.points.key = Some(want_key.clone());
            self.points.pages.clear();
            if !(self.points.shelf == Shelf::YourGames && want_key.1.is_empty()) {
                self.points_more(None);
            }
        }

        if !self.points.balance_asked && ctx.state.steam_id != 0 {
            self.points.balance_asked = true;
            ctx.send(client::Command::LoadPointsBalance);
        }

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(w::text_bold(t!("Points Shop"), 22.0, t.text));
                ui.label(w::text(t!("Make your profile and chats your own with Points Shop items."), 13.0, t.text_dim));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::button(ui, t, Some(icons::EXTERNAL), t!("Open on Steam"), w::Kind::Secondary, w::Size::Sm)
                    .on_hover_text(t!("Your points balance is on Steam's Points Shop page (opens in your browser)."))
                    .clicked()
                {
                    let url = match self.points.shelf {
                        Shelf::Game(id) => format!("https://store.steampowered.com/points/shop/app/{id}"),
                        _ => "https://store.steampowered.com/points/shop".into(),
                    };
                    crate::links::open_http(&url);
                }
                if let Some(pts) = ctx.state.points_balance {
                    w::pill(
                        ui,
                        &format!("{}  {} {}", icons::SPARKLE, fmt_points(pts), t!("points")),
                        t.accent,
                        t.accent.gamma_multiply(0.15),
                    );
                }
            });
        });
        ui.add_space(12.0);

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            // Sidebar
            ui.allocate_ui_with_layout(vec2(210.0, ui.available_height()), Layout::top_down(Align::Min), |ui| {
                w::panel(t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for (heading, entries) in groups() {
                        ui.label(w::caption_text(t, &heading.to_uppercase()));
                        ui.add_space(4.0);
                        for (shelf, label) in entries {
                            let selected = self.points.shelf == shelf;
                            if list_row(ui, t, label, selected).clicked() {
                                self.points.shelf = shelf;
                            }
                        }
                        ui.add_space(10.0);
                    }
                });
            });

            // Main grid
            ui.vertical(|ui| {
                if self.points.shelf == Shelf::YourGames && self.points.key.as_ref().is_some_and(|(_, a)| a.is_empty()) {
                    w::empty(ui, t, icons::GIFT, t!("No games found"), t!("Sign in to browse Points Shop items from games you own."));
                    return;
                }
                let pages: Vec<Option<Result<RewardPage, String>>> = self
                    .points
                    .pages
                    .iter()
                    .map(|p| match &*p.lock().unwrap() {
                        Load::Loading => None,
                        Load::Ready(r) => Some(Ok(r.clone())),
                        Load::Failed(e) => Some(Err(e.clone())),
                    })
                    .collect();

                if let Some(Some(Err(e))) = pages.first() {
                    w::empty(ui, t, icons::WARNING, t!("The Points Shop didn't load"), e);
                    ui.add_space(8.0);
                    if w::button(ui, t, Some(icons::REFRESH), t!("Try again"), w::Kind::Secondary, w::Size::Md).clicked() {
                        self.points.key = None;
                    }
                    return;
                }

                let items: Vec<RewardItem> = pages.iter().filter_map(|p| p.as_ref()?.as_ref().ok()).flat_map(|p| p.items.clone()).collect();
                if items.is_empty() {
                    if pages.iter().any(|p| p.is_none()) {
                        w::loading(ui, t, t!("Loading…"));
                    } else {
                        w::empty(ui, t, icons::GIFT, t!("Nothing here yet"), t!("Steam has no items of this kind right now."));
                    }
                    return;
                }

                let gap = 14.0;
                let width = ui.available_width();
                let per = (((width + gap) / (210.0 + gap)).floor() as usize).max(1);
                let cw = ((width - gap * (per as f32 - 1.0)) / per as f32).floor();
                for row in items.chunks(per) {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for it in row {
                            if reward_card(ui, t, it, cw).clicked() {
                                crate::links::open_http(&format!("https://store.steampowered.com/points/shop/app/{}", it.app_id));
                            }
                        }
                    });
                    ui.add_space(gap);
                }

                match pages.last() {
                    Some(None) => w::loading(ui, t, t!("Loading…")),
                    Some(Some(Ok(p))) if p.next.is_some() => {
                        let next = p.next.clone();
                        ui.vertical_centered(|ui| {
                            if w::button(ui, t, Some(icons::CARET_DOWN), t!("Show more"), w::Kind::Secondary, w::Size::Md).clicked() {
                                self.points_more(next);
                            }
                        });
                    }
                    _ => {}
                }
            });
        });
    }
}

fn list_row(ui: &mut Ui, t: &Theme, label: &str, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, t.corner_small(), w::active_fill(t));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, t.corner_small(), t.surface_hover);
    }
    let color = if selected { t.accent } else { t.text };
    ui.painter().text(pos2(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER, label, w::font_medium(12.5), color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn reward_card(ui: &mut Ui, t: &Theme, it: &RewardItem, width: f32) -> egui::Response {
    let pad = 10.0;
    let img = (width - pad * 2.0).max(60.0);
    let h = pad * 2.0 + img + 76.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let fill = if resp.hovered() { t.surface_hover } else { t.surface };
    ui.painter().rect_filled(rect, t.corner_small(), fill);
    ui.painter().rect_stroke(rect, t.corner_small(), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);

    let pic = Rect::from_min_size(rect.min + vec2(pad, pad), vec2(img, img));
    if it.image.is_empty() || !w::paint_cover(ui, pic, &it.image, CornerRadius::same(4)) {
        if it.image.is_empty() {
            w::placeholder(ui, t, pic, &it.name, CornerRadius::same(4));
        } else {
            w::skeleton(ui, t, pic, CornerRadius::same(4));
        }
    }

    let body = Rect::from_min_max(pos2(pic.left(), pic.bottom() + 8.0), pos2(pic.right(), rect.bottom() - pad));
    let mut b = ui.new_child(UiBuilder::new().max_rect(body).layout(Layout::top_down(Align::Min)));
    b.spacing_mut().item_spacing.y = 4.0;
    b.add(egui::Label::new(w::text_semibold(&it.name, 13.5, t.text)).truncate().selectable(false));
    b.add(egui::Label::new(w::text(kind_name(it.class), 11.5, t.text_dim)).truncate().selectable(false));
    b.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(w::text_bold(icons::SPARKLE, 12.5, t.accent));
        ui.label(w::text_semibold(fmt_points(it.cost), 13.0, t.text));
    });

    let resp = if it.description.is_empty() { resp } else { resp.on_hover_text(&it.description) };
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}
