//! The Community home: built around what players do, in three sections:
//!
//! - **Friends activity**: your friends' screenshots, artwork, reviews,
//!   achievements, new games and posts, as a feed, plus posting your own status.
//! - **Explore**: the community-wide feed of popular and recent screenshots,
//!   artwork, broadcasts, videos, Workshop items, guides and reviews.
//! - **Developers**: news from the developers of the games you play, and
//!   announcements of games you follow.
//!
//! Also includes the Discussions landing and links to Aurora's Friends & Chat
//! page, Workshop, Market listings, and Broadcasts.
//!
//! Ported and adapted from upstream SteamInRust (PR #101 & PR #107 by madonchik123).

mod demo_data;

pub use demo_data::events as demo_events;

use crate::builtin::hub::{self, post};
use client::feed::{ExploreQuery, Post, PostKind, Section};
use client::hub::{News, Request};
use client::{Command, Event, State};
use egui::{Align, Align2, Color32, CornerRadius, Layout, Margin, Rect, Sense, Stroke, Ui, pos2, vec2};
use extensions::{Ctx, Extension, Manifest, PageSpec};
use i18n::t;
use model::{AppId, art};
use std::collections::HashSet;
use ui::widgets::EditMenu;
use ui::{Theme, icons, widgets as w};

/// The page's id.
pub const PAGE: &str = "community_home";

/// How many days of friends' activity show at first (one page is a day).
const FIRST_DAYS: usize = 7;
/// Games whose news the Developers section reads.
const NEWS_GAMES: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Part {
    #[default]
    Friends,
    Explore,
    Developers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarTab {
    Home,
    Discussions,
    Workshop,
    Market,
    Broadcasts,
    FriendsChat,
}

/// Filters of the friends feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FriendFilter {
    #[default]
    All,
    Screenshots,
    Artwork,
    Workshop,
    Reviews,
    Achievements,
    Games,
    Posts,
}

impl FriendFilter {
    const ALL: [FriendFilter; 8] = [
        FriendFilter::All,
        FriendFilter::Screenshots,
        FriendFilter::Artwork,
        FriendFilter::Workshop,
        FriendFilter::Reviews,
        FriendFilter::Achievements,
        FriendFilter::Games,
        FriendFilter::Posts,
    ];

    fn label(self) -> String {
        match self {
            FriendFilter::All => t!("All"),
            FriendFilter::Screenshots => t!("Screenshots"),
            FriendFilter::Artwork => t!("Artwork"),
            FriendFilter::Workshop => t!("Workshop"),
            FriendFilter::Reviews => t!("Reviews"),
            FriendFilter::Achievements => t!("Achievements"),
            FriendFilter::Games => t!("New games"),
            FriendFilter::Posts => t!("Posts"),
        }
        .to_string()
    }

    fn keeps(self, p: &Post) -> bool {
        match self {
            // Game announcements belong to Developers.
            FriendFilter::All => p.kind != PostKind::Announcement,
            FriendFilter::Screenshots => matches!(p.kind, PostKind::Screenshot | PostKind::Video),
            FriendFilter::Artwork => p.kind == PostKind::Artwork,
            FriendFilter::Workshop => matches!(p.kind, PostKind::Workshop | PostKind::Guide),
            FriendFilter::Reviews => p.kind == PostKind::Review,
            FriendFilter::Achievements => p.kind == PostKind::Achievement,
            FriendFilter::Games => p.kind == PostKind::Purchase,
            FriendFilter::Posts => matches!(p.kind, PostKind::Status | PostKind::Other),
        }
    }
}

fn section_label(s: Section) -> String {
    match s {
        Section::All => t!("All"),
        Section::Screenshots => t!("Screenshots"),
        Section::Artwork => t!("Artwork"),
        Section::Broadcasts => t!("Broadcasts"),
        Section::Videos => t!("Videos"),
        Section::Workshop => t!("Workshop"),
        Section::Guides => t!("Guides"),
        Section::Reviews => t!("Reviews"),
    }
    .to_string()
}

/// Which news the Developers section shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum DevFilter {
    #[default]
    All,
    Played,
    Followed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum View {
    #[default]
    Home,
    Discussions,
}

#[derive(Default)]
pub struct CommunityHome {
    view: View,
    part: Part,
    friend_filter: FriendFilter,
    explore_section: Section,
    explore_recent: bool,
    /// Pages of the community-wide feed shown.
    explore_pages: u32,
    /// Days of friends' activity shown.
    days: usize,
    dev_filter: DevFilter,
    /// Requests already sent (until "Try again").
    asked: std::collections::HashMap<String, std::time::Instant>,
    search: String,
    bar_search: String,
    /// Scroll back to the top on the next frame.
    to_top: bool,
    /// The status you're writing for your friends.
    status: StatusBox,
    /// Optional runtime/context for native Steam Broadcast HLS playback.
    rt: Option<tokio::runtime::Handle>,
    egui: Option<egui::Context>,
    /// Active in-app Steam Broadcast player modal.
    broadcast: Option<BroadcastModal>,
}

enum BroadcastState {
    Loading(std::sync::Arc<std::sync::Mutex<Option<Result<BroadcastStream, String>>>>),
    Playing { stream: BroadcastStream, player: video::live::LivePlayer },
    Failed(String),
}

struct BroadcastStream {
    hls_url: String,
    auth_params: String,
    title: String,
    viewers: u64,
}

struct BroadcastModal {
    steam_id: u64,
    page_url: String,
    state: BroadcastState,
}

/// Extract the broadcaster SteamID64 from a Steam broadcast URL
/// (`https://steamcommunity.com/broadcast/watch/7656119...`).
fn broadcast_steam_id(url: &str) -> Option<u64> {
    let parsed = reqwest::Url::parse(url.trim()).ok()?;
    if parsed.scheme() != "https" || !parsed.host_str().is_some_and(|h| h.eq_ignore_ascii_case("steamcommunity.com")) {
        return None;
    }
    let rest = parsed.path().strip_prefix("/broadcast/watch/")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let id: u64 = digits.parse().ok()?;
    (id > 0).then_some(id)
}

async fn fetch_broadcast_stream(steam_id: u64) -> Result<BroadcastStream, String> {
    static HTTP: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(12))
            .user_agent("Mozilla/5.0 Aurora")
            .build()
            .unwrap_or_default()
    });
    let url = format!("https://steamcommunity.com/broadcast/getbroadcastmpd/?steamid={steam_id}&broadcastid=0&viewertoken=0&sessionid=");
    let v: serde_json::Value = HTTP
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let hls = v
        .get("hls_url")
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            let status = v.get("success").and_then(|s| s.as_str()).unwrap_or("offline");
            format!("Broadcast stream unavailable ({status})")
        })?;
    let hls = video::live::validate_live_hls_url(hls)?;
    let auth_params = v
        .get("cdn_auth_url_parameters")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let title = v.get("title").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let viewers = v
        .get("num_viewers")
        .or_else(|| v.get("num_view"))
        .and_then(|n| n.as_u64().or_else(|| n.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(0);
    Ok(BroadcastStream { hls_url: hls, auth_params, title, viewers })
}

/// The "post a status" box at the top of your friends' activity, like
/// Steam's activity page has.
#[derive(Default)]
struct StatusBox {
    text: String,
    /// The game it's tagged with.
    game: Option<(AppId, String)>,
    /// Typed in the game menu's search.
    game_search: String,
    /// Waiting for Steam; the post to show once it's in.
    sending: Option<Post>,
    error: Option<String>,
    /// What you posted, at the top of the feed until Steam's feed has it.
    posted: Vec<Post>,
    /// Reading the feed again after a post: `posted` goes once it's back.
    refreshing: bool,
}

impl CommunityHome {
    pub fn new(rt: tokio::runtime::Handle, egui: egui::Context) -> Self {
        Self { rt: Some(rt), egui: Some(egui), ..Default::default() }
    }

    fn open_broadcast(&mut self, steam_id: u64, page_url: String) {
        let Some(rt) = self.rt.clone() else {
            crate::links::open_http(&page_url);
            return;
        };
        let egui = self.egui.clone();
        let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
        let slot_bg = slot.clone();
        rt.spawn(async move {
            let res = fetch_broadcast_stream(steam_id).await;
            *slot_bg.lock().unwrap() = Some(res);
            if let Some(c) = egui {
                c.request_repaint();
            }
        });
        self.broadcast = Some(BroadcastModal { steam_id, page_url, state: BroadcastState::Loading(slot) });
    }

    fn send(&mut self, ctx: &mut Ctx, key: String, cmd: impl FnOnce() -> Command) {
        let retry = std::time::Duration::from_secs(15);
        if self.asked.get(&key).is_none_or(|sent| sent.elapsed() >= retry) {
            self.asked.retain(|_, sent| sent.elapsed() < retry);
            self.asked.insert(key, std::time::Instant::now());
            ctx.send(cmd());
        }
    }

    fn open(&mut self, ctx: &mut Ctx, click: post::Click) {
        match click {
            post::Click::Open(url) => {
                if let Some(id) = broadcast_steam_id(&url) {
                    self.open_broadcast(id, url);
                    return;
                }
                match hub::route(&url) {
                    Some(v) => {
                        let (page, arg) = v.to_page();
                        ctx.navigate(&page, arg);
                    }
                    None => {
                        crate::links::open_http(&url);
                    }
                }
            }
            post::Click::Hub(app) => ctx.navigate("hub", Some(app.to_string())),
            post::Click::Store(app) => ctx.navigate("store", Some(format!("app:{app}"))),
            post::Click::Profile(id) => ctx.navigate("profile", Some(id.to_string())),
            post::Click::News(app) => ctx.navigate("hub", Some(format!("{app}/news"))),
        }
    }
}

impl Extension for CommunityHome {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "community_home",
            name: "Community",
            author: "Aurora",
            description: "Friends activity, community content and developer news",
            core: true,
            ..Manifest::DEFAULT
        }
    }

    fn pages(&self) -> Vec<PageSpec> {
        vec![PageSpec::new(PAGE, "Community", icons::USERS_THREE).main().order(30)]
    }

    fn on_navigate(&mut self, _page: &str, arg: Option<&str>) {
        self.to_top = true;
        match arg.unwrap_or_default() {
            "discussions" => self.view = View::Discussions,
            "broadcasts" => {
                self.view = View::Home;
                self.part = Part::Explore;
                self.explore_section = Section::Broadcasts;
                self.explore_pages = 1;
            }
            "explore" => (self.view, self.part) = (View::Home, Part::Explore),
            "developers" => (self.view, self.part) = (View::Home, Part::Developers),
            "friends" => (self.view, self.part) = (View::Home, Part::Friends),
            _ => self.view = View::Home,
        }
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Ctx) {
        match event {
            // Signed in again (or as someone else): load the feeds afresh.
            Event::LoggedIn { .. } => self.asked.clear(),
            Event::Hub(client::hub::Response::StatusPosted { result }) => {
                if self.status_answered(result) {
                    ctx.toast(t!("Status posted"));
                }
            }
            Event::Hub(client::hub::Response::Activity { start: 0, result: Ok(_) }) if self.status.refreshing => {
                self.status.refreshing = false;
                self.status.posted.clear();
            }
            _ => {}
        }
    }

    fn page(&mut self, _page: &str, ui: &mut Ui, ctx: &mut Ctx) {
        let t = ctx.theme.clone();
        if self.days == 0 {
            self.days = FIRST_DAYS;
        }
        if self.explore_pages == 0 {
            self.explore_pages = 1;
        }
        let bar_tab = match (self.view, self.part, self.explore_section) {
            (View::Discussions, ..) => BarTab::Discussions,
            (View::Home, Part::Explore, Section::Broadcasts) => BarTab::Broadcasts,
            _ => BarTab::Home,
        };
        let mut scroll = egui::ScrollArea::vertical().id_salt(("community-home", self.view as u8)).auto_shrink([false, false]);
        if std::mem::take(&mut self.to_top) {
            scroll = scroll.vertical_scroll_offset(0.0);
        }
        scroll.show(ui, |ui| {
            if self.view == View::Home {
                self.paint_hero_bg(ui, &t, ctx);
            }
            if let Some(q) = page_bar(ui, &t, ctx, bar_tab, &mut self.bar_search) {
                self.search = q;
            }
            match self.view {
                View::Home => self.home(ui, &t, ctx),
                View::Discussions => self.discussions(ui, &t, ctx),
            }
        });
        self.broadcast_modal(ui, &t);
    }
}

impl CommunityHome {
    fn broadcast_modal(&mut self, ui: &mut Ui, t: &Theme) {
        let Some(modal) = self.broadcast.as_mut() else { return };
        if let BroadcastState::Loading(slot) = &modal.state {
            let ready = slot.lock().unwrap().take();
            if let Some(res) = ready {
                match res {
                    Ok(stream) => {
                        let player = video::live::LivePlayer::start_with_auth(
                            ui.ctx(),
                            stream.hls_url.clone(),
                            stream.auth_params.clone(),
                        );
                        modal.state = BroadcastState::Playing { stream, player };
                    }
                    Err(e) => {
                        modal.state = BroadcastState::Failed(e);
                    }
                }
            }
        }
        let mut close = false;
        let screen = ui.ctx().content_rect();
        let wdt = (screen.width() - 80.0).clamp(480.0, 960.0);
        let hgt = (wdt * 9.0 / 16.0 + 116.0).min(screen.height() - 80.0);
        w::modal(ui.ctx(), egui::Id::unique("community-broadcast-modal"))
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(t.chrome)
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(t.corner())
                    .inner_margin(Margin::same(16))
                    .show(ui, |ui| {
                    ui.set_width(wdt);
                    ui.set_min_height(hgt);
                    ui.horizontal(|ui| {
                        let is_live = match &modal.state {
                            BroadcastState::Playing { player, .. } => player.is_live(),
                            _ => true,
                        };
                        let pill_bg = if is_live { t.danger } else { t.surface };
                        let pill_fg = if is_live { Color32::WHITE } else { t.text_dim };
                        let pill_label = match &modal.state {
                            BroadcastState::Playing { player, .. } if player.is_paused() => t!("Paused").to_string(),
                            BroadcastState::Playing { player, .. } if !player.is_live() => format!("-{}s", player.behind_secs().round() as u32),
                            _ => t!("LIVE").to_string(),
                        };
                        w::pill(ui, &pill_label, pill_fg, pill_bg);
                        let heading = match &modal.state {
                            BroadcastState::Playing { stream, .. } if !stream.title.is_empty() => stream.title.as_str(),
                            _ => t!("Steam Broadcast"),
                        };
                        ui.add(egui::Label::new(w::text_semibold(heading, 15.0, t.text)).truncate());
                        if let BroadcastState::Playing { stream, .. } = &modal.state
                            && stream.viewers > 0
                        {
                            ui.label(w::text(t!("{n} watching", n = stream.viewers), 12.5, t.text_dim));
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if w::icon_button(ui, t, icons::X, t!("Close"), false, 0).clicked() {
                                close = true;
                            }
                            if w::button(ui, t, Some(icons::EXTERNAL), t!("Open on Steam"), w::Kind::Ghost, w::Size::Sm).clicked() {
                                crate::links::open_http(&modal.page_url);
                            }
                        });
                    });
                    ui.add_space(8.0);
                    let vid_h = (hgt - 102.0).max(240.0);
                    match &mut modal.state {
                        BroadcastState::Loading(_) => {
                            ui.allocate_ui(vec2(wdt, vid_h), |ui| {
                                ui.centered_and_justified(|ui| {
                                    w::loading(ui, t, t!("Connecting to Steam Broadcast…"));
                                });
                            });
                        }
                        BroadcastState::Playing { player, .. } => {
                            if let Some(err) = player.error() {
                                ui.allocate_ui(vec2(wdt, vid_h), |ui| {
                                    ui.vertical_centered(|ui| {
                                        ui.add_space(40.0);
                                        w::empty(ui, t, icons::WARNING, t!("Couldn't play broadcast"), &err);
                                    });
                                });
                            } else if let Some(tex) = player.texture(ui.ctx()) {
                                let (rect, resp) = ui.allocate_exact_size(vec2(wdt, vid_h), Sense::click());
                                ui.painter().rect_filled(rect, t.corner_small(), Color32::BLACK);
                                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                                ui.painter().image(tex, rect, uv, Color32::WHITE);
                                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                                    || ui.input(|i| i.key_pressed(egui::Key::Space))
                                {
                                    player.toggle_pause();
                                }
                                if player.is_paused() {
                                    ui.painter().circle_filled(rect.center(), 30.0, Color32::from_black_alpha(160));
                                    ui.painter().text(rect.center() + vec2(2.0, 0.0), Align2::CENTER_CENTER, icons::PLAY, w::font(26.0), Color32::WHITE);
                                }
                            } else {
                                ui.allocate_ui(vec2(wdt, vid_h), |ui| {
                                    ui.centered_and_justified(|ui| {
                                        w::loading(ui, t, t!("Connecting to Steam Broadcast…"));
                                    });
                                });
                            }
                            ui.add_space(8.0);
                            // Bottom transport bar: play/pause, -10s/+10s rewind, DVR scrubber, LIVE, mute & volume.
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                let paused = player.is_paused();
                                let play_icon = if paused { icons::PLAY } else { "\u{E39E}" };
                                let play_tip = if paused { t!("Play") } else { t!("Pause") };
                                if w::icon_button(ui, t, play_icon, play_tip, paused, 0).clicked() {
                                    player.toggle_pause();
                                }
                                let buffered = player.buffered_secs();
                                let behind = player.behind_secs();
                                if ui
                                    .add_enabled_ui(buffered >= 1.0, |ui| {
                                        w::icon_button(ui, t, icons::ARROW_LEFT, t!("Rewind 10s"), false, 0)
                                    })
                                    .inner
                                    .clicked()
                                {
                                    player.rewind(10.0);
                                }
                                if ui
                                    .add_enabled_ui(behind > 0.15, |ui| {
                                        w::icon_button(ui, t, icons::ARROW_RIGHT, t!("Forward 10s"), false, 0)
                                    })
                                    .inner
                                    .clicked()
                                {
                                    player.forward(10.0);
                                }
                                let muted = player.is_muted();
                                let mut vol = if muted { 0.0 } else { player.volume() };
                                let vol_icon = if muted || vol <= 0.01 {
                                    icons::SPEAKER_X
                                } else if vol < 0.5 {
                                    "\u{E44C}"
                                } else {
                                    icons::SPEAKER_HIGH
                                };
                                let vol_tip = if muted { t!("Unmute") } else { t!("Mute") };
                                if w::icon_button(ui, t, vol_icon, vol_tip, muted, 0).clicked() {
                                    player.toggle_mute();
                                }
                                ui.spacing_mut().slider_width = 84.0;
                                if ui.add(egui::Slider::new(&mut vol, 0.0..=1.0).show_value(false)).changed() {
                                    player.set_volume(vol);
                                }
                                // DVR timeline scrubber fills remaining space before Back to Live button.
                                let max_buf = buffered.max(1.0);
                                let mut pos = (max_buf - behind).clamp(0.0, max_buf);
                                let scrub_w = (ui.available_width() - 120.0).max(80.0);
                                ui.spacing_mut().slider_width = scrub_w;
                                if ui.add_enabled(buffered >= 1.0, egui::Slider::new(&mut pos, 0.0..=max_buf).show_value(false)).changed() {
                                    player.seek_behind((max_buf - pos).max(0.0));
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let kind = if player.is_live() { w::Kind::Ghost } else { w::Kind::Primary };
                                    if w::button(ui, t, None, t!("Back to live"), kind, w::Size::Sm).clicked() {
                                        player.jump_to_live();
                                    }
                                });
                            });
                        }
                        BroadcastState::Failed(err) => {
                            ui.allocate_ui(vec2(wdt, vid_h), |ui| {
                                ui.vertical_centered(|ui| {
                                    ui.add_space(40.0);
                                    w::empty(ui, t, icons::WARNING, t!("Couldn't play broadcast"), err);
                                    ui.add_space(10.0);
                                    if w::button(ui, t, Some(icons::EXTERNAL), t!("Open on Steam"), w::Kind::Primary, w::Size::Md).clicked() {
                                        crate::links::open_http(&modal.page_url);
                                    }
                                });
                            });
                        }
                    }
                    let _ = modal.steam_id;
                });
            });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        if close {
            self.broadcast = None;
        }
    }
}

/// Top sub-navigation bar for Community Home: merged seamlessly into the page.
fn page_bar(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, current: BarTab, search: &mut String) -> Option<String> {
    let mut out = None;
    egui::Frame::new().inner_margin(Margin::symmetric(20, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            let tabs = [
                (BarTab::Home, t!("Home"), PAGE, None),
                (BarTab::Discussions, t!("Discussions"), PAGE, Some("discussions")),
                (BarTab::Workshop, t!("Workshop"), "workshop", None),
                (BarTab::Market, t!("Market"), "inventory", Some("listings")),
                (BarTab::Broadcasts, t!("Broadcasts"), PAGE, Some("broadcasts")),
                (BarTab::FriendsChat, t!("Friends & Chat"), "community", None),
            ];
            for (tab, label, page, arg) in tabs {
                if w::chip(ui, t, label, current == tab).clicked() {
                    ctx.navigate(page, arg.map(String::from));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let width = ui.available_width().clamp(160.0, 320.0);
                let r = w::search_sized(ui, t, search, t!("Search for a game's community…"), width, 30.0);
                if !search.trim().is_empty() {
                    let matches = game_matches(ctx.state, search, 8);
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if let Some((app, _)) = matches.first() {
                            ctx.navigate("hub", Some(app.to_string()));
                            search.clear();
                        }
                    }
                    if r.has_focus() {
                        out = Some(search.clone());
                    }
                    let id = egui::Id::unique("community-bar-search");
                    egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(r.rect.left_bottom() + vec2(0.0, 4.0)).show(ui.ctx(), |ui| {
                        search_menu(ui, t, ctx, search, r.rect.width().max(220.0), &matches);
                    });
                }
            });
        });
    });
    out
}

fn search_menu(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, search: &mut String, width: f32, matches: &[(AppId, &str)]) {
    egui::Frame::new()
        .fill(t.chrome)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(t.corner_small())
        .shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(120) })
        .inner_margin(Margin::same(6))
        .show(ui, |ui| {
            ui.set_width((width - 12.0).max(160.0));
            w::menu_spacing(ui);
            if matches.is_empty() {
                ui.label(w::text(t!("No games found"), 13.0, t.text_dim));
            }
            for (app, name) in matches {
                if w::menu_item(ui, t, icons::USERS_THREE, name).clicked() {
                    ctx.navigate("hub", Some(app.to_string()));
                    search.clear();
                }
            }
        });
}

/// Games whose name has `query`: yours first (most played), then the rest
/// of your library.
fn game_matches<'a>(state: &'a State, query: &str, max: usize) -> Vec<(AppId, &'a str)> {
    let q = query.trim().to_lowercase();
    if q.is_empty() || max == 0 {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<_> = my_games(state).into_iter()
        .filter(|(app, name)| model::contains_ignore_case(name, &q) && seen.insert(*app))
        .take(max)
        .collect();
    if out.len() < max && let Some(apps) = &state.apps {
        let mut rest: Vec<_> = apps.values()
            .filter(|a| !a.is_dlc() && model::contains_ignore_case(&a.name, &q))
            .map(|a| (a.app_id, a.name.as_str()))
            .collect();
        rest.sort_by_key(|(_, name)| *name);
        let remaining = max - out.len();
        out.extend(rest.into_iter().filter(|(app, _)| seen.insert(*app)).take(remaining));
    }
    out
}

/// Your games, most played recently first: Steam's list when it came,
/// else your library.
fn my_games(state: &State) -> Vec<(AppId, &str)> {
    if let Some(Ok(list)) = state.player_games.get(&state.steam_id) && !list.is_empty() {
        let mut list: Vec<_> = list.iter().collect();
        list.sort_by_key(|g| (std::cmp::Reverse(g.playtime_2weeks), std::cmp::Reverse(g.last_played.unwrap_or(0)), std::cmp::Reverse(g.playtime_minutes)));
        return list.into_iter().map(|g| (g.app_id, g.name.as_str())).collect();
    }
    let mut apps: Vec<_> = state.apps.as_ref().map(|a| a.values().filter(|a| !a.is_dlc() && a.kind == "game")
        .map(|a| (a.app_id, a.name.as_str())).collect()).unwrap_or_default();
    apps.sort_by_key(|(_, name)| *name);
    apps
}

// ---------- home ----------

impl CommunityHome {
    fn home(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        self.hero(ui, t, ctx);
        w::centered(ui, 1564.0, 20.0, |ui| {
            ui.spacing_mut().item_spacing.y = 16.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for (part, label) in [(Part::Friends, t!("Friends activity")), (Part::Explore, t!("Explore")), (Part::Developers, t!("Developers"))] {
                    if w::nav_tab(ui, t, label, self.part == part, 34.0).clicked() {
                        self.part = part;
                    }
                }
            });
            match self.part {
                Part::Friends => self.friends(ui, t, ctx),
                Part::Explore => self.explore(ui, t, ctx),
                Part::Developers => self.developers(ui, t, ctx),
            }
            ui.add_space(40.0);
        });
    }

    /// Paint the hero game-art collage from the very top of the scroll area
    /// so the Community sub-navigation bar blends seamlessly into the banner.
    fn paint_hero_bg(&self, ui: &mut Ui, t: &Theme, ctx: &Ctx) {
        let full = ui.available_width();
        let rect = Rect::from_min_size(ui.cursor().min, vec2(full, 296.0));
        let games: Vec<AppId> = my_games(ctx.state).into_iter().map(|g| g.0).take(4).collect();
        let pieces = [(0.0, 0.0, 0.37, 0.9), (0.34, 0.0, 0.36, 0.8), (0.57, 0.0, 0.28, 0.96), (0.83, 0.0, 0.2, 0.87)];
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        for (i, app) in games.iter().enumerate() {
            let (x, y, wf, hf) = pieces[i];
            let r = Rect::from_min_size(rect.min + vec2(rect.width() * x, rect.height() * y), vec2(rect.width() * wf, rect.height() * hf)).intersect(rect);
            w::paint_cover(ui, r, &art::hero(*app), CornerRadius::ZERO);
        }
        painter.rect_filled(rect, CornerRadius::ZERO, t.bg.gamma_multiply(0.76));
        w::fade_down(ui, rect, t.bg, 0.0);
    }

    /// The welcome heading and game community search inside the hero banner.
    fn hero(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let full = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(full, 224.0), Sense::hover());
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let title = t!("Welcome to the Community. Search for a game to get started.");
        painter.text(pos2(rect.center().x, rect.top() + 68.0), Align2::CENTER_CENTER, title, w::font_bold(22.0), t.text);
        let sw = 514.0f32.min(full - 40.0).max(120.0);
        let sr = Rect::from_min_size(pos2(rect.center().x - sw / 2.0, rect.top() + 112.0), vec2(sw, 44.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(sr).layout(Layout::left_to_right(Align::Center)));
        let r = w::search_sized(&mut child, t, &mut self.search, t!("Search…"), sw, 44.0);
        if !self.search.trim().is_empty() {
            let matches = game_matches(ctx.state, &self.search, 6);
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if let Some((app, _)) = matches.first() {
                    ctx.navigate("hub", Some(app.to_string()));
                    self.search.clear();
                }
            }
            let id = egui::Id::unique("community-hero-search");
            egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(r.rect.left_bottom() + vec2(0.0, 4.0)).show(ui.ctx(), |ui| {
                search_menu(ui, t, ctx, &mut self.search, sw, &matches);
            });
        }
    }

    /// A row of filter chips in a bar, and optionally a "Viewing" sort on the right.
    fn filter_row<T: PartialEq + Copy>(ui: &mut Ui, t: &Theme, value: &mut T, options: &[(T, String)], right: impl FnOnce(&mut Ui)) -> bool {
        let mut changed = false;
        egui::Frame::new().fill(t.surface).corner_radius(t.corner_small()).inner_margin(Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for (opt, label) in options {
                    if w::chip(ui, t, label, *value == *opt).clicked() && *value != *opt {
                        *value = *opt;
                        changed = true;
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), right);
            });
        });
        changed
    }

    // ----- friends -----

    fn friends(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        if ctx.state.steam_id != 0 {
            self.status_box(ui, t, ctx);
        }
        let options: Vec<(FriendFilter, String)> = FriendFilter::ALL.iter().map(|f| (*f, f.label())).collect();
        Self::filter_row(ui, t, &mut self.friend_filter, &options, |_| {});
        if ctx.state.steam_id == 0 {
            w::empty(ui, t, icons::USERS, t!("Sign in to see your friends"), t!("Your friends' screenshots, reviews and achievements show here."));
            return;
        }
        // What you just posted, on top until Steam's feed has it.
        let mine: Vec<Post> = self.status.posted.iter().rev().filter(|p| self.friend_filter.keeps(p)).cloned().collect();
        // Day pages, newest first, until we have the days asked for.
        let lang = i18n::language().steam_name().to_string();
        let mut posts: Vec<&Post> = mine.iter().collect();
        let mut seen: HashSet<String> = mine.iter().map(post::key).collect();
        let mut start = 0u64;
        let mut loading = false;
        let mut failed: Option<String> = None;
        let mut more = true;
        let mut shown_days = 0;
        let state = ctx.state;
        while shown_days < self.days {
            match state.hub.activity.get(&start) {
                None => {
                    let l = lang.clone();
                    self.send(ctx, format!("activity:{start}"), || Command::Hub(Request::Activity { start, language: l }));
                    loading = true;
                    break;
                }
                Some(Err(e)) => {
                    failed = Some(e.clone());
                    break;
                }
                Some(Ok(feed)) => {
                    // Steam can list the same event on two days' pages: once here.
                    for p in feed.posts.iter().filter(|p| self.friend_filter.keeps(p)) {
                        if seen.insert(post::key(p)) {
                            posts.push(p);
                        }
                    }
                    shown_days += 1;
                    if feed.next == 0 {
                        more = false;
                        break;
                    }
                    start = feed.next;
                }
            }
        }
        // A quiet week: keep reading back a little further on its own.
        if !loading && failed.is_none() && more && posts.len() < 6 && self.days < 30 && shown_days >= self.days {
            self.days += FIRST_DAYS;
            ui.ctx().request_repaint();
        }
        if posts.is_empty() {
            if let Some(e) = failed {
                if error(ui, t, &e) {
                    self.asked.retain(|k, _| !k.starts_with("activity:"));
                }
            } else if loading {
                skeletons(ui, t);
            } else {
                w::empty(ui, t, icons::USERS, t!("Nothing new from your friends"), t!("When your friends share screenshots, write reviews or unlock achievements, it shows here."));
            }
            return;
        }
        if let Some(c) = post::masonry(ui, t, state, &posts, "friends", true) {
            self.open(ctx, c);
        }
        if loading {
            w::loading(ui, t, t!("Loading…"));
        } else if let Some(e) = failed {
            if error(ui, t, &e) {
                self.asked.retain(|k, _| !k.starts_with("activity:"));
            }
        } else if more {
            ui.vertical_centered(|ui| {
                if w::button(ui, t, Some(icons::CARET_DOWN), t!("Show older activity"), w::Kind::Secondary, w::Size::Md).clicked() {
                    self.days += FIRST_DAYS;
                }
            });
        }
    }

    /// "Post a status for your friends…": your avatar, the text, a game to
    /// tag it with, and Post (Steam's activity page has the same).
    fn status_box(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let me = ctx.state.persona(ctx.state.steam_id);
        let sending = self.status.sending.is_some();
        let mut submit = false;
        egui::Frame::new()
            .fill(t.surface)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(t.corner())
            .inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    w::avatar(ui, t, &art::avatar(me.avatar_hash.as_deref()), 40.0, None);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        let id = egui::Id::unique("community-status-text");
                        let focused = ui.memory(|m| m.has_focus(id));
                        if focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)) {
                            submit = true;
                        }
                        let border = if focused { t.accent.gamma_multiply(0.6) } else { t.border };
                        egui::Frame::new()
                            .fill(t.raised)
                            .stroke(Stroke::new(1.0, border))
                            .corner_radius(t.corner_small())
                            .inner_margin(Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                ui.add_enabled(
                                    !sending,
                                    egui::TextEdit::multiline(&mut self.status.text)
                                        .id(id)
                                        .hint_text(w::text(t!("Post a status for your friends…"), 13.5, t.text_dim))
                                        .font(w::font(13.5))
                                        .text_color(t.text)
                                        .frame(egui::Frame::NONE)
                                        .desired_rows(2)
                                        .char_limit(1000)
                                        .desired_width(f32::INFINITY),
                                )
                                .with_edit_menu();
                            });
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            self.game_menu(ui, t, ctx.state);
                            ui.label(w::text(t!("Your friends see what you post here."), 12.0, t.text_faint));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let ready = !self.status.text.trim().is_empty() && !sending;
                                let label = if sending { t!("Posting…") } else { t!("Post") };
                                let posted = ui.add_enabled_ui(ready, |ui| w::button(ui, t, Some(icons::SEND), label, w::Kind::Primary, w::Size::Md).clicked()).inner;
                                submit |= posted;
                            });
                        });
                        if let Some(e) = &self.status.error {
                            ui.label(w::text(e, 12.0, t.danger));
                        }
                    });
                });
            });
        if submit {
            self.post_status(ctx);
        }
    }

    /// "Tag with game": your games, with a search.
    fn game_menu(&mut self, ui: &mut Ui, t: &Theme, state: &State) {
        let label = match &self.status.game {
            Some((_, name)) => name.clone(),
            None => t!("Tag with game").to_string(),
        };
        let r = w::button(ui, t, Some(icons::CONTROLLER), &format!("{label}  {}", icons::CARET_DOWN), w::Kind::Secondary, w::Size::Sm);
        egui::Popup::menu(&r).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).width(280.0).show(|ui| {
            let width = ui.available_width();
            w::search(ui, t, &mut self.status.game_search, t!("Find a game"), width);
            ui.add_space(4.0);
            w::menu_spacing(ui);
            if w::menu_item(ui, t, icons::X, t!("No game")).clicked() {
                self.status.game = None;
                ui.close();
            }
            let q = self.status.game_search.trim().to_lowercase();
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (app, name) in my_games(state).into_iter().filter(|(_, n)| q.is_empty() || n.to_lowercase().contains(&q)).take(40) {
                    if w::menu_item(ui, t, icons::CONTROLLER, &name).clicked() {
                        self.status.game = Some((app, name.to_owned()));
                        ui.close();
                    }
                }
            });
        });
    }

    /// Send the status (in `--demo`, it's only added here).
    fn post_status(&mut self, ctx: &mut Ctx) {
        let demo = std::env::args().any(|a| a == "--demo");
        if let Some(req) = self.submit(ctx.state, demo) {
            ctx.send(Command::Hub(req));
        }
    }

    /// Take the status as written: in the demo it's posted here at once;
    /// else it waits for Steam and the request to send comes back.
    fn submit(&mut self, state: &State, demo: bool) -> Option<Request> {
        let text = self.status.text.trim().to_string();
        if text.is_empty() || self.status.sending.is_some() {
            return None;
        }
        let me = state.persona(state.steam_id);
        let (app_id, game) = self.status.game.clone().unwrap_or_default();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let post = Post {
            kind: PostKind::Status,
            author: state.steam_id,
            author_name: me.name.clone(),
            avatar: Some(art::avatar(me.avatar_hash.as_deref())),
            app_id,
            game: game.clone(),
            action: if app_id != 0 { t!("posted a status about {game}", game = game) } else { String::new() },
            text: text.clone(),
            when: t!("Today").to_string(),
            time: now,
            ..Default::default()
        };
        self.status.error = None;
        if demo {
            self.status.posted.push(post);
            self.status.text.clear();
            self.status.game = None;
            return None;
        }
        self.status.sending = Some(post);
        Some(Request::PostStatus { text, app_id })
    }

    /// Steam answered a posted status. True when it's in.
    fn status_answered(&mut self, result: &Result<(), String>) -> bool {
        let post = self.status.sending.take();
        match result {
            Ok(()) => {
                self.status.posted.extend(post);
                self.status.text.clear();
                self.status.game = None;
                self.status.error = None;
                // Steam's feed has it now: read it again.
                self.status.refreshing = true;
                self.asked.retain(|k, _| !k.starts_with("activity:"));
                true
            }
            Err(e) => {
                self.status.error = Some(t!("Couldn't post your status: {error}", error = i18n::tr_dyn(e)));
                false
            }
        }
    }

    // ----- explore -----

    fn explore(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let options: Vec<(Section, String)> = Section::ALL.iter().map(|s| (*s, section_label(*s))).collect();
        let mut recent = self.explore_recent;
        let changed = Self::filter_row(ui, t, &mut self.explore_section, &options, |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // Right to left: the last one first.
            if w::chip(ui, t, t!("Most Recent"), recent).clicked() {
                recent = true;
            }
            if w::chip(ui, t, t!("Most Popular"), !recent).clicked() {
                recent = false;
            }
            ui.add_space(6.0);
            ui.label(w::text(t!("Viewing"), 13.5, t.text_dim));
        });
        if changed || recent != self.explore_recent {
            self.explore_recent = recent;
            self.explore_pages = 1;
        }
        let lang = i18n::language().steam_name().to_string();
        let state = ctx.state;
        let mut posts: Vec<&Post> = Vec::new();
        let mut loading = false;
        let mut failed = None;
        let mut last_full = false;
        for page in 1..=self.explore_pages {
            let q = ExploreQuery { section: self.explore_section, recent: self.explore_recent, page, language: lang.clone(), ..Default::default() };
            match state.hub.explore.get(&q.key()) {
                None => {
                    self.send(ctx, format!("explore:{}", q.key()), || Command::Hub(Request::Explore(q)));
                    loading = true;
                    break;
                }
                Some(Err(e)) => {
                    failed = Some(e.clone());
                    break;
                }
                Some(Ok(feed)) => {
                    last_full = !feed.posts.is_empty();
                    // The same post can come back on the next page.
                    for p in &feed.posts {
                        if !posts.iter().any(|x| x.url == p.url && x.file_id == p.file_id) {
                            posts.push(p);
                        }
                    }
                }
            }
        }
        if posts.is_empty() {
            if let Some(e) = failed {
                if error(ui, t, &e) {
                    self.asked.retain(|k, _| !k.starts_with("explore:"));
                }
            } else if loading {
                skeletons(ui, t);
            } else if self.explore_section == Section::Broadcasts {
                w::empty(ui, t, icons::MONITOR, t!("No one is broadcasting right now"), t!("Check back later."));
            } else {
                w::empty(ui, t, icons::USERS_THREE, t!("Nothing here yet"), t!("Check back later."));
            }
            return;
        }
        if let Some(c) = post::masonry(ui, t, state, &posts, "explore", true) {
            self.open(ctx, c);
        }
        if loading {
            w::loading(ui, t, t!("Loading…"));
        } else if failed.is_none() && last_full && self.explore_pages < 10 {
            ui.vertical_centered(|ui| {
                if w::button(ui, t, Some(icons::CARET_DOWN), t!("See more"), w::Kind::Secondary, w::Size::Md).clicked() {
                    self.explore_pages += 1;
                }
            });
        }
    }

    // ----- developers -----

    fn developers(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let options = [
            (DevFilter::All, t!("All").to_string()),
            (DevFilter::Played, t!("Games you play").to_string()),
            (DevFilter::Followed, t!("Games you follow").to_string()),
        ];
        Self::filter_row(ui, t, &mut self.dev_filter, &options, |_| {});
        let state = ctx.state;
        if state.steam_id == 0 {
            w::empty(ui, t, hub::NEWSPAPER, t!("Sign in to see news"), t!("News from the developers of your games shows here."));
            return;
        }
        if !state.player_games.contains_key(&state.steam_id) {
            let (me, lang) = (state.steam_id, i18n::language().steam_name().to_string());
            self.send(ctx, "player-games".into(), || Command::LoadPlayerGames { steam_id: me, language: lang });
        }
        let mut news: Vec<Post> = Vec::new();
        let mut loading = false;
        if self.dev_filter != DevFilter::Followed {
            for (app, name) in my_games(state).into_iter().take(NEWS_GAMES) {
                match state.hub.news.get(&app) {
                    None => {
                        self.send(ctx, format!("news:{app}"), || Command::Hub(Request::News { app_id: app, count: 3 }));
                        loading = true;
                    }
                    Some(Ok(list)) => news.extend(list.iter().map(|n| news_post(app, &name, n))),
                    Some(Err(_)) => {}
                }
            }
        }
        if self.dev_filter != DevFilter::Played {
            // Announcements of games you follow come in your activity feed.
            let lang = i18n::language().steam_name().to_string();
            match state.hub.activity.get(&0) {
                None => {
                    self.send(ctx, "activity:0".into(), || Command::Hub(Request::Activity { start: 0, language: lang }));
                    loading = true;
                }
                Some(Ok(_)) => {
                    for feed in state.hub.activity.values().flatten() {
                        for p in feed.posts.iter().filter(|p| p.kind == PostKind::Announcement) {
                            if !news.iter().any(|n| n.url == p.url) {
                                news.push(p.clone());
                            }
                        }
                    }
                }
                Some(Err(_)) => {}
            }
        }
        news.sort_by_key(|p| std::cmp::Reverse(p.time));
        if news.is_empty() {
            if loading {
                skeletons(ui, t);
            } else {
                w::empty(ui, t, hub::NEWSPAPER, t!("No news yet"), t!("News from the developers of your games shows here."));
            }
            return;
        }
        let refs: Vec<&Post> = news.iter().take(40).collect();
        if let Some(c) = post::masonry(ui, t, state, &refs, "developers", true) {
            self.open(ctx, c);
        }
        if loading {
            w::loading(ui, t, t!("Loading…"));
        }
    }

    // ----- discussions -----

    /// The Discussions landing: your games' forums.
    fn discussions(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        ui.add_space(36.0);
        ui.vertical_centered(|ui| {
            ui.label(w::text_bold(t!("Welcome to the discussions"), 24.0, t.text));
            ui.add_space(8.0);
            ui.label(w::text(t!("Pick a forum to start the conversation."), 15.0, t.text_dim));
        });
        ui.add_space(32.0);
        let state = ctx.state;
        if state.steam_id != 0 && !state.player_games.contains_key(&state.steam_id) {
            let (me, lang) = (state.steam_id, i18n::language().steam_name().to_string());
            self.send(ctx, "player-games".into(), || Command::LoadPlayerGames { steam_id: me, language: lang });
        }
        let games = my_games(state);
        w::centered(ui, 1564.0, 20.0, |ui| {
            ui.label(w::text_bold(t!("Your game forums"), 16.0, t.text));
            ui.add_space(8.0);
            if games.is_empty() {
                w::empty(ui, t, icons::CHATS, t!("No games yet"), t!("The forums of the games you own show here."));
                return;
            }
            let gap = 16.0;
            let cols = ((ui.available_width() + gap) / (360.0 + gap)).floor().clamp(1.0, 4.0) as usize;
            let cw = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
            for row in games.iter().take(24).collect::<Vec<_>>().chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for (app, name) in row {
                        if forum_tile(ui, t, state, *app, name, cw).clicked() {
                            ctx.navigate("discussions", Some(app.to_string()));
                        }
                    }
                });
                ui.add_space(8.0);
            }
            ui.add_space(32.0);
            w::card(t).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 10.0;
                ui.label(w::text_bold(t!("About discussions"), 16.0, t.text_dim));
                for (bold, rest) in [
                    (t!("Everyone is invited!"), t!("The discussions are for everyone, new and advanced players alike.")),
                    (t!("Searching is key!"), t!("Before you post a question, search the forum to see whether it has been answered.")),
                    (t!("Be kind."), t!("If someone is spamming or harassing, report the post and let the moderators take a look.")),
                ] {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.label(w::text_bold(bold, 14.0, t.text));
                        ui.label(w::text(rest, 14.0, t.text));
                    });
                }
            });
        });
        ui.add_space(40.0);
    }
}

/// A forum in the Discussions landing: the game's icon, name and a caret.
fn forum_tile(ui: &mut Ui, t: &Theme, state: &State, app: AppId, name: &str, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 72.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    ui.painter().rect_filled(rect, t.corner(), w::mix(t.surface, t.surface_hover, hover));
    let icon = Rect::from_center_size(pos2(rect.left() + 36.0, rect.center().y), vec2(32.0, 32.0));
    let hash = state.apps.as_ref().and_then(|a| a.get(&app)).and_then(|o| o.icon.clone());
    let drawn = hash.is_some_and(|h| w::paint_cover(ui, icon, &art::icon(app, &h), t.corner_small()));
    if !drawn && !w::paint_cover(ui, icon, &art::capsule(app), t.corner_small()) {
        ui.painter().rect_filled(icon, t.corner_small(), t.raised);
    }
    let text_rect = Rect::from_min_max(pos2(icon.right() + 18.0, rect.top()), pos2(rect.right() - 40.0, rect.bottom()));
    let mut job = egui::text::LayoutJob::simple_singleline(name.to_string(), w::font(14.5), t.text);
    job.wrap.max_width = text_rect.width();
    job.wrap.max_rows = 1;
    job.wrap.overflow_character = Some('…');
    let g = ui.painter().layout_job(job);
    ui.painter().galley(pos2(text_rect.left(), rect.center().y - g.size().y / 2.0), g, t.text);
    ui.painter().text(pos2(rect.right() - 22.0, rect.center().y), Align2::CENTER_CENTER, icons::CARET_RIGHT, w::font(16.0), t.text_dim);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A game's news post as a feed post. Opens in the game's hub.
fn news_post(app: AppId, game: &str, n: &News) -> Post {
    let doc = hub::rich::parse(&n.contents);
    let image = doc.blocks.iter().find_map(|b| if let hub::rich::Block::Image(u) = b { Some(u.clone()) } else { None });
    // The words of the post's paragraphs (not its pictures' links).
    let plain: String = doc
        .blocks
        .iter()
        .filter_map(|b| if let hub::rich::Block::Para(spans) = b { Some(spans.iter().map(|s| s.text.as_str()).collect::<String>()) } else { None })
        .filter(|p| !p.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Post {
        kind: PostKind::Announcement,
        app_id: app,
        game: game.to_string(),
        author_name: if n.author.is_empty() { n.feed.clone() } else { n.author.clone() },
        title: n.title.clone(),
        text: plain.chars().take(400).collect(),
        images: vec![image.unwrap_or_else(|| art::header(app))],
        url: n.url.clone(),
        time: n.time,
        ..Default::default()
    }
}

/// Grey boxes while the first posts load.
fn skeletons(ui: &mut Ui, t: &Theme) {
    let gap = 16.0;
    let w2 = (ui.available_width() - gap) / 2.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for h in [420.0, 300.0] {
            let (r, _) = ui.allocate_exact_size(vec2(w2, h), Sense::hover());
            w::skeleton(ui, t, r, t.corner());
        }
    });
}

/// An error and "Try again"; true when clicked.
fn error(ui: &mut Ui, t: &Theme, msg: &str) -> bool {
    let mut again = false;
    ui.vertical_centered(|ui| {
        ui.add_space(40.0);
        ui.label(w::text(icons::WARNING, 34.0, t.text_faint));
        ui.label(w::text(i18n::tr_dyn(msg), 13.5, t.text_dim));
        again = w::button(ui, t, Some(icons::REFRESH), t!("Try again"), w::Kind::Secondary, w::Size::Sm).clicked();
    });
    again
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_in() -> State {
        let mut state = State::default();
        state.steam_id = 76561197960265729;
        state
    }

    #[test]
    fn a_demo_status_shows_at_once() {
        let mut home = CommunityHome::default();
        home.status.text = "  one more round?  ".into();
        home.status.game = Some((570, "Dota 2".into()));
        assert!(home.submit(&signed_in(), true).is_none());
        assert_eq!(home.status.posted.len(), 1);
        let p = &home.status.posted[0];
        assert_eq!((p.kind, p.app_id, p.text.as_str(), p.author), (PostKind::Status, 570, "one more round?", 76561197960265729));
        assert!(home.status.text.is_empty() && home.status.game.is_none());
        // Nothing to post: nothing happens.
        assert!(home.submit(&signed_in(), true).is_none());
        assert_eq!(home.status.posted.len(), 1);
    }

    #[test]
    fn a_status_waits_for_steam() {
        let mut home = CommunityHome::default();
        home.status.text = "gg".into();
        match home.submit(&signed_in(), false) {
            Some(Request::PostStatus { text, app_id }) => assert_eq!((text.as_str(), app_id), ("gg", 0)),
            other => panic!("expected a post, got {other:?}"),
        }
        // Sent once, kept until Steam answers.
        assert!(home.submit(&signed_in(), false).is_none());
        assert!(home.status.posted.is_empty() && home.status.text == "gg");
        // A refusal keeps the text and says why.
        assert!(!home.status_answered(&Err("You've been posting too frequently".into())));
        assert_eq!(home.status.text, "gg");
        assert!(home.status.error.is_some());
        // Then it goes through: on top of the feed, the feed read again.
        home.asked.insert("activity:0".into(), std::time::Instant::now());
        assert!(home.submit(&signed_in(), false).is_some());
        assert!(home.status_answered(&Ok(())));
        assert_eq!(home.status.posted.len(), 1);
        assert!(home.status.text.is_empty() && home.status.refreshing);
        assert!(!home.asked.contains_key("activity:0"));
    }

    #[test]
    fn friend_filters_sort_posts() {
        let p = |kind| Post { kind, ..Default::default() };
        assert!(FriendFilter::All.keeps(&p(PostKind::Review)));
        assert!(!FriendFilter::All.keeps(&p(PostKind::Announcement)));
        assert!(FriendFilter::Screenshots.keeps(&p(PostKind::Screenshot)));
        assert!(!FriendFilter::Screenshots.keeps(&p(PostKind::Artwork)));
        assert!(FriendFilter::Workshop.keeps(&p(PostKind::Guide)));
        assert!(FriendFilter::Games.keeps(&p(PostKind::Purchase)));
        assert!(FriendFilter::Posts.keeps(&p(PostKind::Status)));
    }

    #[test]
    fn news_becomes_a_post() {
        let n = News {
            id: "1".into(),
            title: "Patch".into(),
            contents: "[img]https://x/a.png[/img]Fixed [b]bugs[/b].".into(),
            feed: "Announcements".into(),
            time: 5,
            ..Default::default()
        };
        let p = news_post(620, "Portal 2", &n);
        assert_eq!((p.kind, p.app_id, p.title.as_str(), p.time), (PostKind::Announcement, 620, "Patch", 5));
        assert_eq!(p.images, ["https://x/a.png"]);
        assert!(p.text.contains("Fixed") && p.text.contains("bugs") && !p.text.contains("[b]") && !p.text.contains("https://"), "{}", p.text);
        assert_eq!(p.author_name, "Announcements");
        assert_eq!(p.url, "");
    }

    #[test]
    fn demo_events_populate_activity_and_explore() {
        let mut state = State::default();
        for e in demo_events() {
            state.apply(&e);
        }
        let act = state.hub.activity.get(&0).expect("demo activity loaded").as_ref().unwrap();
        assert!(!act.posts.is_empty());
        let lang = i18n::language().steam_name().to_string();
        for section in Section::ALL {
            let key = ExploreQuery { section, recent: false, page: 1, language: lang.clone(), ..Default::default() }.key();
            assert!(state.hub.explore.get(&key).is_some_and(|r| r.is_ok()), "missing demo explore for {section:?}");
        }
    }

    #[test]
    fn community_tab_sits_next_to_library() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let ctx = egui::Context::default();
        let flags = std::sync::Arc::new(std::sync::Mutex::new(crate::builtin::settings::AppFlags::default()));
        let themes = std::sync::Arc::new(std::sync::Mutex::new(crate::app::ThemeState {
            manager: themes::ThemeManager::new(std::env::temp_dir(), std::env::temp_dir()),
            hot_reload: false,
            last_error: None,
            dirty: false,
            revision: 0,
        }));
        let layout = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let reg = crate::builtin::registry(rt.handle().clone(), ctx, flags, themes, layout);
        let main_ids: Vec<String> = reg
            .pages()
            .into_iter()
            .filter(|p| p.placement == extensions::Placement::Main)
            .map(|p| p.id)
            .collect();
        assert_eq!(main_ids, vec!["store", "library", PAGE, "profile"]);
    }

    #[test]
    fn broadcast_steam_id_only_accepts_steamcommunity_https() {
        assert_eq!(broadcast_steam_id("https://steamcommunity.com/broadcast/watch/76561198000000001"), Some(76561198000000001));
        assert_eq!(broadcast_steam_id("http://steamcommunity.com/broadcast/watch/76561198000000001"), None);
        assert_eq!(broadcast_steam_id("https://evil.example/broadcast/watch/76561198000000001"), None);
        assert_eq!(broadcast_steam_id("https://steamcommunity.com.evil.example/broadcast/watch/76561198000000001"), None);
    }
}
