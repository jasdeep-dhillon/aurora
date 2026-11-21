//! A post in a social feed: the Community home's Friends activity, Explore,
//! and Developers sections all draw their posts with [`card`], laid out two to
//! a row by [`masonry`].
//!
//! Read-only: the counts under a post open it, they don't vote.
//!
//! Ported and adapted from upstream SteamInRust (PR #101 by madonchik123).

use client::State;
use client::feed::{Post, PostKind};
use egui::{Align, Align2, Color32, CornerRadius, Layout, Margin, Rect, Sense, Stroke, Ui, pos2, vec2};
use i18n::t;
use model::{AppId, SteamId, art};
use ui::{Theme, icons, widgets as w};

const SMILEY: &str = "\u{E436}";

/// What was clicked in a post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Click {
    /// The post itself (a Steam link: a shared file, a review, an
    /// announcement, a broadcast).
    Open(String),
    /// The game's community hub.
    Hub(AppId),
    /// The game's store page.
    Store(AppId),
    Profile(SteamId),
    /// The game's news (an announcement).
    News(AppId),
}

/// What kind of post it is, for the line under Explore cards.
pub fn kind_label(kind: PostKind) -> String {
    match kind {
        PostKind::Screenshot => t!("Screenshot"),
        PostKind::Artwork => t!("Artwork"),
        PostKind::Video => t!("Video"),
        PostKind::Broadcast => t!("Broadcast"),
        PostKind::Workshop => t!("Workshop item"),
        PostKind::Guide => t!("Guide"),
        PostKind::Review => t!("Review"),
        PostKind::Announcement => t!("Announcement"),
        PostKind::Discussion => t!("Discussion"),
        PostKind::Achievement | PostKind::Purchase | PostKind::Status | PostKind::Other => return String::new(),
    }
    .to_string()
}

/// Whether a post has anything to show (never draw an empty card).
pub fn showable(p: &Post) -> bool {
    p.kind == PostKind::Review || !p.title.trim().is_empty() || !p.text.trim().is_empty() || !p.images.is_empty() || !p.action.trim().is_empty()
}

/// Where a click on the post itself goes.
fn open_target(p: &Post) -> Option<Click> {
    // Announcements read best in the game's news (in the app).
    if p.kind == PostKind::Announcement && p.app_id != 0 {
        return Some(Click::News(p.app_id));
    }
    (!p.url.is_empty()).then(|| Click::Open(p.url.clone())).or((p.app_id != 0).then_some(Click::Hub(p.app_id)))
}

/// The game's name for a post (Steam's, else the one we know).
fn game_name(state: &State, p: &Post) -> String {
    if !p.game.is_empty() {
        return p.game.clone();
    }
    state.app_name(p.app_id).unwrap_or_default().to_string()
}

/// "Posted on 12 Sep 2023", "Live now", or the day Steam gave.
fn when(p: &Post) -> String {
    if p.live {
        return t!("Live now").to_string();
    }
    if p.time > 0 {
        return t!("Posted on {date}", date = ui::date(p.time));
    }
    p.when.trim_start_matches("Posted: ").to_string()
}

/// 50600 -> "50,600".
pub fn number(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The post's main picture and its height for `width`.
fn picture(p: &Post, width: f32) -> Option<(String, f32)> {
    let url = match p.kind {
        PostKind::Review | PostKind::Status | PostKind::Achievement | PostKind::Other => return None,
        // A purchase shows the game's header, not Steam's small capsule.
        PostKind::Purchase if p.app_id != 0 => art::header(p.app_id),
        PostKind::Guide => return None,
        _ => p.images.first()?.clone(),
    };
    let ratio = match p.kind {
        PostKind::Purchase => 215.0 / 460.0,
        PostKind::Announcement => 0.45,
        _ => 402.0 / 737.0,
    };
    Some((url, (width * ratio).round()))
}

/// A rough height for a post before it was drawn once (the masonry then
/// uses the real one).
fn guess_height(p: &Post, width: f32) -> f32 {
    let mut h = 30.0 + 29.0 + 20.0 + 36.0 + 37.0;
    if let Some((_, ph)) = picture(p, width) {
        h += ph;
    }
    let chars = (width / 8.0).max(20.0);
    let lines = p.text.lines().map(|l| (l.chars().count() as f32 / chars).ceil().max(1.0)).sum::<f32>().min(14.0);
    h + lines * 19.0
}

fn hover_frame<R>(ui: &mut Ui, t: &Theme, id: egui::Id, add: impl FnOnce(&mut Ui) -> R) -> (R, egui::Response) {
    // Claim last frame's rect before drawing children so interactive controls
    // inside the card keep their own clicks.
    let last: Option<Rect> = ui.data(|d| d.get_temp(id));
    let behind = last.map(|r| ui.interact(r, id, Sense::click()));
    let hovered = behind.as_ref().is_some_and(|r| r.contains_pointer());
    let anim = ui.ctx().animate_bool_with_time(id.with("hover"), hovered, 0.12);
    let fill = w::mix(t.surface, t.surface_hover, anim * 0.45);
    let border = w::mix(t.border, t.accent.gamma_multiply(0.5), anim);
    let inner = egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(t.corner())
        .inner_margin(Margin::same(16))
        .show(ui, add);
    let rect = inner.response.rect;
    if last != Some(rect) {
        ui.data_mut(|d| d.insert_temp(id, rect));
        ui.ctx().request_repaint();
    }
    let resp = match behind {
        Some(r) => r.on_hover_cursor(egui::CursorIcon::PointingHand),
        None => ui.interact(Rect::NOTHING, id.with("first"), Sense::hover()),
    };
    (inner.inner, resp)
}

/// One post, `width` wide.
/// `show_game`: the game's name and links on top.
/// `id`: the card's own (see [`post_ids`]); it must differ between cards.
pub fn card(ui: &mut Ui, t: &Theme, state: &State, p: &Post, width: f32, show_game: bool, id: egui::Id) -> Option<Click> {
    let mut click = None;
    let inner = (width - 32.0).max(60.0);
    let (_, frame) = ui.push_id(id, |ui| {
        hover_frame(ui, t, id, |ui| {
            ui.set_width(inner);
            ui.spacing_mut().item_spacing.y = 12.0;
            let game = game_name(state, p);
            if show_game && p.app_id != 0 && !game.is_empty() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let (pic, pr) = ui.allocate_exact_size(vec2(62.0, 29.0), Sense::click());
                    if ui.is_rect_visible(pic) && !w::paint_cover(ui, pic, &art::header(p.app_id), t.corner_small()) {
                        if w::image_state(ui, &art::header(p.app_id)) == w::ImageState::Missing {
                            ui.painter().rect_filled(pic, t.corner_small(), t.raised);
                            ui.painter().text(pic.center(), Align2::CENTER_CENTER, icons::CONTROLLER, w::font(16.0), t.text_dim);
                        } else {
                            w::skeleton(ui, t, pic, t.corner_small());
                        }
                    }
                    if pr.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        click = Some(Click::Hub(p.app_id));
                    }
                    let buttons = 32.0 * 2.0 + 16.0;
                    let room = (ui.available_width() - buttons).max(40.0);
                    ui.allocate_ui_with_layout(vec2(room, 29.0), Layout::left_to_right(Align::Center), |ui| {
                        let r = ui.add(egui::Label::new(w::text_bold(&game, 15.0, t.text)).truncate().sense(Sense::click()));
                        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            click = Some(Click::Hub(p.app_id));
                        }
                    });
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if w::icon_button(ui, t, icons::GLOBE, t!("Community hub"), false, 0).clicked() {
                        click = Some(Click::Hub(p.app_id));
                    }
                    if w::icon_button(ui, t, icons::STOREFRONT, t!("Store page"), false, 0).clicked() {
                        click = Some(Click::Store(p.app_id));
                    }
                });
            }
            if let Some(c) = author_row(ui, t, state, p) {
                click = Some(c);
            }
            if let Some(c) = body(ui, t, p, inner) {
                click = Some(c);
            }
            if let Some(c) = footer(ui, t, p) {
                click = Some(c);
            }
        })
    })
    .inner;
    if click.is_none() && frame.clicked() {
        click = open_target(p);
    }
    click
}

/// Avatar, name, what they did; the date on the right.
fn author_row(ui: &mut Ui, t: &Theme, state: &State, p: &Post) -> Option<Click> {
    let mut click = None;
    let persona = (p.author != 0).then(|| state.persona(p.author));
    let name = persona.as_ref().map(|x| x.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| p.author_name.clone());
    let avatar = p.avatar.clone().or_else(|| persona.as_ref().filter(|x| x.avatar_hash.is_some()).map(|x| art::avatar(x.avatar_hash.as_deref())));
    let starts = |s: &str| !s.is_empty() && p.action.starts_with(s);
    let sentence = starts(&name) || starts(&p.author_name);
    let detail = match p.kind {
        PostKind::Broadcast if p.views > 0 => t!("{n} viewers", n = number(p.views)),
        _ if sentence => String::new(),
        _ if p.action.is_empty() => kind_label(p.kind),
        _ => p.action.clone(),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let date = when(p);
        let date_w = if date.is_empty() { 0.0 } else { ui.painter().layout_no_wrap(date.clone(), w::font(12.0), t.text_dim).size().x + 12.0 };
        let room = (ui.available_width() - date_w).max(40.0);
        ui.allocate_ui_with_layout(vec2(room, 22.0), Layout::left_to_right(Align::Center), |ui| {
            if let Some(a) = &avatar {
                let r = w::avatar(ui, t, a, 20.0, None);
                if p.author != 0 && r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    click = Some(Click::Profile(p.author));
                }
            } else if p.kind == PostKind::Announcement {
                ui.label(w::text(super::NEWSPAPER, 13.5, t.text_dim));
            }
            let label = if sentence { p.action.trim_end_matches([':', ' ']).to_string() } else { name };
            if !label.is_empty() {
                let r = ui.add(egui::Label::new(w::text_medium(label, 13.5, t.text)).truncate().sense(Sense::click()));
                if p.author != 0 && r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    click = Some(Click::Profile(p.author));
                }
            }
            if !detail.is_empty() {
                ui.add(egui::Label::new(w::text(detail, 13.5, t.text_dim)).truncate());
            }
        });
        if !date.is_empty() {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(w::text(date, 12.0, t.text_dim));
            });
        }
    });
    click
}

fn body(ui: &mut Ui, t: &Theme, p: &Post, width: f32) -> Option<Click> {
    let mut click = None;
    let open = || open_target(p);
    match p.kind {
        PostKind::Review => {
            review_box(ui, t, p);
            if !p.text.is_empty() {
                text_block(ui, t, &p.text, width, 14, 14.5);
            }
        }
        PostKind::Achievement => {
            const SHOWN: usize = 4;
            let names: Vec<&str> = p.text.split("\n\n").map(str::trim).filter(|s| !s.is_empty()).collect();
            let count = p.images.len().max(names.len());
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                for i in 0..count.min(SHOWN) {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 12.0;
                        let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                        match p.images.get(i) {
                            Some(img) if ui.is_rect_visible(r) => w::image_or_skeleton(ui, t, r, img, t.corner_small()),
                            Some(_) => {}
                            None => {
                                ui.painter().rect_filled(r, t.corner_small(), t.raised);
                                ui.painter().text(r.center(), Align2::CENTER_CENTER, icons::TROPHY, w::font(20.0), t.text_dim);
                            }
                        }
                        let entry = names.get(i).copied().unwrap_or_default();
                        let mut lines = entry.lines();
                        let name = lines.next().unwrap_or_default().to_string();
                        let desc = lines.collect::<Vec<_>>().join(" ");
                        ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), Layout::top_down(Align::Min), |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            if !name.is_empty() {
                                ui.add(egui::Label::new(w::text_medium(name, 13.5, t.text)).wrap());
                            }
                            if !desc.is_empty() {
                                ui.add(egui::Label::new(w::text(desc, 12.0, t.text_dim)).wrap());
                            }
                        });
                    });
                }
                let hidden = count.saturating_sub(SHOWN) as u32 + p.more;
                if hidden > 0 {
                    ui.label(w::text(t!("+{n} more", n = hidden), 12.0, t.text_dim));
                }
            });
        }
        _ => {
            if let Some((url, h)) = picture(p, width) {
                let (r, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
                if ui.is_rect_visible(r) && !w::paint_cover(ui, r, &url, t.corner_small()) {
                    w::skeleton(ui, t, r, t.corner_small());
                }
                if matches!(p.kind, PostKind::Video | PostKind::Broadcast) {
                    play_mark(ui, t, r);
                }
                if p.live {
                    live_badge(ui, t, r);
                }
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    click = open();
                }
            }
            match p.kind {
                PostKind::Announcement | PostKind::Guide | PostKind::Discussion => {
                    if !p.title.is_empty() {
                        let r = ui.add(egui::Label::new(w::text_bold(&p.title, 15.5, t.text)).wrap().sense(Sense::click()));
                        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            click = open();
                        }
                    }
                    if !p.text.is_empty() {
                        ui.add_space(-4.0);
                        text_block(ui, t, &p.text, width, 5, 13.5);
                    }
                }
                PostKind::Status | PostKind::Other | PostKind::Purchase => {
                    if !p.text.is_empty() {
                        text_block(ui, t, &p.text, width, 8, 13.5);
                    }
                }
                _ => {}
            }
        }
    }
    click
}

/// Up to `rows` lines of text, cut with "…".
fn text_block(ui: &mut Ui, t: &Theme, text: &str, width: f32, rows: usize, size: f32) {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), w::font(size), t.text, width);
    job.wrap.max_rows = rows;
    job.wrap.overflow_character = Some('…');
    let galley = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
    ui.painter().galley(r.min, galley, t.text);
}

/// The review's verdict: thumb, "Recommends it" / "Not Recommended", hours.
fn review_box(ui: &mut Ui, t: &Theme, p: &Post) {
    let up = p.recommended.unwrap_or(true);
    egui::Frame::new()
        .fill(t.raised)
        .corner_radius(t.corner_small())
        .inner_margin(Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let (r, _) = ui.allocate_exact_size(vec2(46.0, 46.0), Sense::hover());
                ui.painter().rect_filled(r, t.corner_small(), if up { t.accent_2 } else { t.danger });
                ui.painter().text(r.center(), Align2::CENTER_CENTER, if up { icons::THUMBS_UP } else { icons::THUMBS_DOWN }, w::font(26.0), Color32::WHITE);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.add_space(3.0);
                    ui.label(w::text_semibold(if up { t!("Recommends it") } else { t!("Not Recommended") }, 13.5, t.text));
                    if !p.hours.is_empty() {
                        ui.label(w::text(&p.hours, 12.5, t.text_dim));
                    }
                });
            });
        });
}

fn play_mark(ui: &Ui, t: &Theme, r: Rect) {
    let b = Rect::from_center_size(r.center(), vec2(58.0, 58.0));
    ui.painter().rect_filled(b, t.corner_small(), Color32::from_black_alpha(150));
    ui.painter().text(b.center(), Align2::CENTER_CENTER, icons::PLAY, w::font(26.0), Color32::WHITE);
}

/// "● LIVE" in the corner of a broadcast.
fn live_badge(ui: &Ui, t: &Theme, r: Rect) {
    let g = ui.painter().layout_no_wrap(t!("LIVE").to_string(), w::font_bold(12.0), t.danger);
    let size = vec2(g.size().x + 28.0, 26.0);
    let b = Rect::from_min_size(r.min + vec2(9.0, 10.0), size);
    ui.painter().rect_filled(b, t.corner_small(), t.raised);
    ui.painter().circle_filled(pos2(b.left() + 13.0, b.center().y), 5.0, t.danger);
    ui.painter().galley(pos2(b.left() + 22.0, b.center().y - g.size().y / 2.0), g, t.danger);
}

fn stat_chip(ui: &mut Ui, t: &Theme, icon: &str, label: Option<&str>, count: &str) -> egui::Response {
    let text = match label {
        Some(l) => format!("{icon}  {l} ({count})"),
        None => format!("{icon}  {count}"),
    };
    let galley = ui.painter().layout_no_wrap(text, w::font_medium(12.5), Color32::PLACEHOLDER);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 18.0, 26.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    ui.painter().rect_filled(rect, t.corner_small(), w::mix(t.raised, t.surface_hover, hover));
    let c = w::mix(t.text_dim, t.text, hover);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, c);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The caption and the counts; for a review "Was this review helpful?" and its counts.
fn footer(ui: &mut Ui, t: &Theme, p: &Post) -> Option<Click> {
    let mut click = None;
    let open = open_target(p);
    if p.kind == PostKind::Review {
        let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().rect_filled(line, CornerRadius::ZERO, t.border);
        ui.add_space(-4.0);
        ui.label(w::text(t!("Was this review helpful?"), 12.0, t.text_dim));
        ui.add_space(-4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for (icon, label, n) in [
                (icons::THUMBS_UP, Some(t!("Yes")), p.votes_up),
                (SMILEY, Some(t!("Funny")), p.funny),
                (icons::CHAT, None, p.comments),
            ] {
                if stat_chip(ui, t, icon, label, &number(n)).clicked() {
                    click = open.clone();
                }
            }
        });
        return click;
    }
    let caption = match p.kind {
        PostKind::Screenshot | PostKind::Artwork | PostKind::Video | PostKind::Workshop => p.title.clone(),
        _ => String::new(),
    };
    let show_counts = p.votes_up > 0 || p.comments > 0 || p.file_id != 0;
    if caption.is_empty() && !show_counts {
        ui.add_space(-12.0);
        return None;
    }
    ui.horizontal(|ui| {
        if show_counts {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let mut c = None;
                for (icon, n) in [(icons::CHAT, p.comments), (icons::THUMBS_UP, p.votes_up)] {
                    if stat_chip(ui, t, icon, None, &number(n)).clicked() {
                        c = open.clone();
                    }
                }
                if c.is_some() {
                    click = c;
                }
                if !caption.is_empty() {
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        if caption_link(ui, t, &caption) {
                            click = open.clone();
                        }
                    });
                }
            });
        } else if caption_link(ui, t, &caption) {
            click = open.clone();
        }
    });
    click
}

fn caption_link(ui: &mut Ui, t: &Theme, caption: &str) -> bool {
    let r = ui.add(egui::Label::new(w::text_medium(caption, 14.0, t.accent)).truncate().sense(Sense::click()));
    r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// A post's own key: what it is, where it links, who and what.
pub fn key(p: &Post) -> String {
    format!("{:?}|{}|{}|{}|{}|{}|{}|{}", p.kind, p.url, p.file_id, p.title, p.action, p.author, p.when, p.images.first().map(String::as_str).unwrap_or_default())
}

/// Ids for a list of posts: from each post's key, so they stay put while
/// the list grows, with the n-th repeat of the same key told apart. Never
/// two the same, however Steam repeats an event.
pub fn post_ids(salt: &str, posts: &[&Post]) -> Vec<egui::Id> {
    let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    posts
        .iter()
        .map(|p| {
            let k = key(p);
            let n = seen.entry(k.clone()).or_insert(0);
            *n += 1;
            egui::Id::unique(("feed-post", salt, k, *n))
        })
        .collect()
}

/// Posts in two columns (one when narrow), each going under the shorter
/// column, like a social feed. Heights come from the last frame; a card
/// off the screen only takes its space (its pictures aren't fetched until
/// it's near).
pub fn masonry(ui: &mut Ui, t: &Theme, state: &State, posts: &[&Post], salt: &str, show_game: bool) -> Option<Click> {
    let gap = 16.0;
    let total = ui.available_width();
    let cols = if total >= 900.0 { 2 } else { 1 };
    let col_w = ((total - gap * (cols as f32 - 1.0)) / cols as f32).floor();
    let store = egui::Id::unique(("feed-heights", salt, col_w as u32));
    let mut heights: std::collections::HashMap<egui::Id, f32> = ui.data(|d| d.get_temp(store)).unwrap_or_default();
    let ids = post_ids(salt, posts);
    let mut fill = vec![0.0f32; cols];
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); cols];
    for (i, p) in posts.iter().enumerate().filter(|(_, p)| showable(p)) {
        let h = heights.get(&ids[i]).copied().unwrap_or_else(|| guess_height(p, col_w));
        let c = (0..cols).min_by(|a, b| fill[*a].total_cmp(&fill[*b])).unwrap_or(0);
        fill[c] += h + gap;
        columns[c].push(i);
    }
    let mut click = None;
    let near = ui.clip_rect().expand2(vec2(0.0, ui.clip_rect().height()));
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for col in &columns {
            ui.allocate_ui_with_layout(vec2(col_w, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(col_w);
                ui.spacing_mut().item_spacing.y = gap;
                for &i in col {
                    let p = posts[i];
                    let top = ui.cursor().top();
                    if let Some(h) = heights.get(&ids[i]).copied() {
                        let span = Rect::from_min_size(pos2(ui.cursor().left(), top), vec2(col_w, h));
                        if !near.intersects(span) {
                            ui.allocate_space(vec2(col_w, h));
                            continue;
                        }
                    }
                    if let Some(c) = card(ui, t, state, p, col_w, show_game, ids[i]) {
                        click = Some(c);
                    }
                    heights.insert(ids[i], ui.cursor().top() - top - gap);
                }
            });
        }
    });
    ui.data_mut(|d| d.insert_temp(store, heights));
    click
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_post_gets_its_own_id() {
        let ach = Post { kind: PostKind::Achievement, author: 7, action: "Ferris achieved Hades".into(), images: vec!["https://x/a.jpg".into()], when: "17 Nov".into(), ..Default::default() };
        let other = Post { kind: PostKind::Achievement, author: 7, action: "Ferris achieved Hades".into(), images: vec!["https://x/b.jpg".into()], when: "17 Nov".into(), ..Default::default() };
        let status = Post { kind: PostKind::Status, author: 8, action: "Nova and Pixel are now friends.".into(), ..Default::default() };
        let posts = [&ach, &ach, &other, &status, &status, &ach];
        let first = post_ids("friends", &posts);
        let unique: std::collections::HashSet<_> = first.iter().collect();
        assert_eq!(unique.len(), posts.len());
        assert_eq!(first, post_ids("friends", &posts));
        let explore = post_ids("explore", &posts);
        assert!(explore.iter().all(|id| !first.contains(id)));
    }

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(number(5), "5");
        assert_eq!(number(1234), "1,234");
        assert_eq!(number(50600), "50,600");
        assert_eq!(number(1000000), "1,000,000");
    }

    #[test]
    fn pictures_follow_the_kind() {
        let shot = Post { kind: PostKind::Screenshot, images: vec!["https://x/a.jpg".into()], ..Default::default() };
        assert_eq!(picture(&shot, 737.0).map(|(u, h)| (u, h as u32)), Some(("https://x/a.jpg".to_string(), 402)));
        let review = Post { kind: PostKind::Review, images: vec!["https://x/a.jpg".into()], ..Default::default() };
        assert!(picture(&review, 700.0).is_none());
        let bought = Post { kind: PostKind::Purchase, app_id: 620, ..Default::default() };
        assert_eq!(picture(&bought, 460.0).map(|(u, _)| u), Some(art::header(620)));
    }
}
