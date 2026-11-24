//! Big Picture's settings: what you'd want to change from the couch, with a
//! controller. Sections on the left, their settings on the right; A changes
//! the selected one. They're the same settings the normal window edits.
//! Things that need typing, a file dialog or a browser stay there.

use super::focus::{Dir, Place};
use super::{BigPicture, Frame, paint};
use crate::builtin::downloads::{DownloadSettings, Downloads};
use crate::builtin::settings::{AppFlags, FpsCorner};
use client::Command;
use egui::{Align2, Color32, Id, Rect, Ui, pos2, vec2};
use extensions::Action;
use i18n::t;
use model::PersonaState;
use ui::{icons, widgets as w};

/// What Big Picture's settings remember while open.
#[derive(Default)]
pub struct SettingsState {
    section: usize,
    /// The themes to pick from (read once per visit).
    themes: Option<Vec<(Option<String>, String)>>,
    /// "Sign out" was pressed once: it needs a second press before this time.
    confirm_until: f64,
    last_focus: Option<Id>,
}

impl SettingsState {
    /// Open on this section (previews).
    pub fn open_at(&mut self, section: usize) {
        self.section = section.min(sections().len() - 1);
    }
}

pub(super) const NOTIFICATIONS: usize = 3;
pub(super) const CONTROLLER: usize = 4;
pub(super) const OVERLAY: usize = 5;

const SIZES: [f32; 4] = [0.85, 1.0, 1.15, 1.3];
const SPEEDS_KBPS: [u32; 7] = [1_000, 2_500, 5_000, 10_000, 25_000, 50_000, 100_000];
const CONNECTIONS: [u32; 6] = [4, 8, 16, 24, 32, 48];
const STATUSES: [PersonaState; 5] = [PersonaState::Online, PersonaState::Away, PersonaState::Busy, PersonaState::Invisible, PersonaState::Offline];
const CORNERS: [FpsCorner; 5] = [FpsCorner::Off, FpsCorner::TopLeft, FpsCorner::TopRight, FpsCorner::BottomLeft, FpsCorner::BottomRight];

type Act = Box<dyn FnOnce(&mut BigPicture, &mut Frame, &egui::Context)>;

fn act(f: impl FnOnce(&mut BigPicture, &mut Frame, &egui::Context) + 'static) -> Act {
    Box::new(f)
}

enum Row {
    Toggle { label: String, note: String, on: bool, act: Act },
    Choice { label: String, note: String, value: String, act: Act },
    Button { icon: &'static str, label: String, danger: bool, checked: bool, act: Act },
    Info { label: String, value: String },
}

fn toggle(label: &str, note: &str, on: bool, a: Act) -> Row {
    Row::Toggle { label: label.into(), note: note.into(), on, act: a }
}

fn choice(label: &str, note: &str, value: String, a: Act) -> Row {
    Row::Choice { label: label.into(), note: note.into(), value, act: a }
}

/// The next value in a list (the first after the last, or if it's not in it).
fn next_in<T: PartialEq + Copy>(list: &[T], now: T) -> T {
    list[list.iter().position(|x| *x == now).map_or(0, |i| (i + 1) % list.len())]
}

/// Flip one of the app's on/off settings.
fn flag(get: fn(&mut AppFlags) -> &mut bool) -> Act {
    act(move |_, f, _| {
        let flags = f.app.shared_flags();
        let mut l = flags.lock().unwrap();
        let v = get(&mut l);
        *v = !*v;
    })
}

fn controller_change(change: impl FnOnce(&mut client::ControllerSettings) + 'static) -> Act {
    act(move |_, _, _| {
        let mut settings = client::controller_settings();
        change(&mut settings);
        client::set_controller_settings(settings);
    })
}

fn controller_toggle(label: &str, value: bool, field: fn(&mut client::ControllerSettings) -> &mut bool) -> Row {
    toggle(label, "", value, controller_change(move |settings| {
        let current = field(settings);
        *current = !*current;
    }))
}

fn deadzone(label: &str, value: f32, field: fn(&mut client::PadCalibration) -> &mut f32) -> Row {
    choice(label, t!("Select to increase by 5%. After 50%, wraps to 0%."), format!("{:.0}%", value * 100.0),
        controller_change(move |settings| {
            let value = field(&mut settings.calibration);
            let step = (*value * 20.0).round().clamp(0.0, 10.0) as u8;
            *value = f32::from((step + 1) % 11) / 20.0;
        }))
}

fn with_downloads<R>(f: &mut Frame, g: impl FnOnce(&mut Downloads, &mut extensions::Ctx) -> R) -> Option<R> {
    let app = &mut *f.app;
    let env = extensions::Env { state: &app.state, local: &app.local, theme: &app.prefs.theme, zoom: app.prefs.zoom, client: &app.client, actions: &mut *f.actions };
    app.registry.with_ext::<Downloads, R>("downloads", env, g)
}

/// Change the download settings (the Downloads page applies them).
fn dl(change: impl FnOnce(&mut DownloadSettings) + 'static) -> Act {
    act(move |_, f, _| {
        with_downloads(f, |d, cx| {
            let mut s = d.tv_settings();
            change(&mut s);
            d.tv_apply(cx, s);
        });
    })
}

fn sections() -> [(&'static str, &'static str); 8 + cfg!(target_os = "linux") as usize] {
    [
        (icons::MONITOR, t!("General")),
        (icons::DOWNLOAD, t!("Downloads")),
        (icons::BOOKS, t!("Library")),
        (icons::BELL, t!("Notifications")),
        (icons::CONTROLLER, t!("Controller")),
        (icons::STACK, t!("In-game overlay")),
        (icons::PALETTE, t!("Appearance")),
        (icons::USER_CIRCLE, t!("Account")),
        #[cfg(target_os = "linux")]
        (icons::WINDOWS, t!("Compatibility")),
    ]
}

fn level_name(level: u8) -> &'static str {
    match level {
        0 => t!("Low"),
        2 => t!("High"),
        _ => t!("Normal"),
    }
}

fn speed_name(level: u8) -> &'static str {
    match level {
        0 => t!("Slow"),
        2 => t!("Fast"),
        _ => t!("Normal"),
    }
}

fn hour(h: u8) -> String {
    format!("{h:02}:00")
}

/// What the library opens on: 0 installed, 1 runs here, 2 everything.
fn library_name(v: u8) -> &'static str {
    match v {
        1 => t!("Runs on this computer"),
        2 => t!("All games"),
        _ => t!("Installed"),
    }
}

fn rows(bp: &mut BigPicture, f: &mut Frame) -> Vec<Row> {
    let flags = *f.app.shared_flags().lock().unwrap();
    match bp.settings.section {
        1 => {
            let (s, regions) = with_downloads(f, |d, cx| (d.tv_settings(), d.tv_regions(cx))).unwrap_or_default();
            let region = if s.region == "auto" { t!("Automatic (recommended)").to_string() } else { regions.iter().find(|(id, _)| *id == s.region).map(|(_, n)| n.clone()).unwrap_or_else(|| s.region.clone()) };
            let ids: Vec<String> = std::iter::once("auto".to_string()).chain(regions.into_iter().map(|(id, _)| id)).collect();
            let at = ids.iter().position(|i| *i == s.region).unwrap_or(0);
            let next_region = ids[(at + 1) % ids.len()].clone();
            let mut out = vec![
                toggle(t!("Automatic updates"), t!("Keep installed games up to date. Turn off to update only when you choose (or when you launch a game)."), flags.auto_updates, flag(|x| &mut x.auto_updates)),
                toggle(t!("Allow downloads while playing"), "", s.while_playing, dl(|d| d.while_playing = !d.while_playing)),
                choice(t!("Download region"), t!("Measured from this PC: the closest servers usually download fastest."), region, dl(move |d| d.region = next_region)),
                toggle(t!("Limit bandwidth"), "", s.limit_bandwidth, dl(|d| d.limit_bandwidth = !d.limit_bandwidth)),
            ];
            if s.limit_bandwidth {
                let shown = format!("{:.1} MB/s", s.limit_kbps as f32 / 1000.0);
                out.push(choice(t!("Speed limit"), "", shown, dl(|d| d.limit_kbps = next_in(&SPEEDS_KBPS, d.limit_kbps))));
            }
            out.push(toggle(t!("Only auto-update games between"), "", s.schedule, dl(|d| d.schedule = !d.schedule)));
            if s.schedule {
                out.push(choice(t!("From"), "", hour(s.schedule_from), dl(|d| d.schedule_from = (d.schedule_from + 1) % 24)));
                out.push(choice(t!("To"), "", hour(s.schedule_to), dl(|d| d.schedule_to = (d.schedule_to + 1) % 24)));
            }
            out.push(choice(t!("Parallel connections"), "", s.connections.to_string(), dl(|d| d.connections = next_in(&CONNECTIONS, d.connections))));
            out.push(toggle(t!("Show speeds in bits per second"), "", s.show_bits, dl(|d| d.show_bits = !d.show_bits)));
            out
        }
        2 => {
            let v = f.app.prefs.bigpicture_library;
            vec![
                choice(t!("Library opens on"), "", library_name(v).to_string(), act(|bp, f, _| {
                    let next = (f.app.prefs.bigpicture_library + 1) % 3;
                    f.app.prefs.bigpicture_library = next;
                    // The library picks it up on its next draw.
                    bp.installed_only = None;
                })),
                toggle(t!("Steam Cloud"), t!("Enable Steam Cloud for games that support it. Each game can also be turned off in its Properties."), flags.cloud, flag(|x| &mut x.cloud)),
            ]
        }
        3 => vec![
            toggle(t!("New chat messages"), t!("Show a pop-up when a friend messages you."), flags.toast_messages, flag(|x| &mut x.toast_messages)),
            toggle(t!("Steam notifications"), t!("Show a pop-up for gifts, items, friend requests, sales and more."), flags.toast_notifications, flag(|x| &mut x.toast_notifications)),
        ],
        CONTROLLER => {
            let p = &f.app.prefs;
            let (swap, repeat, stick) = (p.bigpicture_swap_ab, p.bigpicture_repeat, p.bigpicture_stick);
            let cfg = client::controller_settings();
            let cal = cfg.calibration;
            let devices = client::poll_controllers();
            let mut out = vec![
                Row::Info { label: t!("Controllers connected").to_string(), value: bp.pads.max(devices.controllers.len()).to_string() },
            ];
            out.extend([
                toggle(t!("Swap A and B"), t!("For controllers that have A and B the other way round."), swap, act(|_, f, _| f.app.prefs.bigpicture_swap_ab = !f.app.prefs.bigpicture_swap_ab)),
                choice(t!("Held button repeat"), t!("How fast the selection moves while you hold a direction."), speed_name(repeat).to_string(), act(|_, f, _| f.app.prefs.bigpicture_repeat = (f.app.prefs.bigpicture_repeat + 1) % 3)),
                choice(t!("Stick sensitivity"), t!("How little you need to push the stick to move the selection."), level_name(stick).to_string(), act(|_, f, _| f.app.prefs.bigpicture_stick = (f.app.prefs.bigpicture_stick + 1) % 3)),
                controller_toggle(t!("Enable Steam Input for Xbox controllers"), cfg.families.xbox, |c| &mut c.families.xbox),
                controller_toggle(t!("Enable Steam Input for PlayStation controllers"), cfg.families.playstation, |c| &mut c.families.playstation),
                controller_toggle(t!("Enable Steam Input for Switch Pro controllers"), cfg.families.switch_pro, |c| &mut c.families.switch_pro),
                controller_toggle(t!("Enable Steam Input for generic controllers"), cfg.families.generic, |c| &mut c.families.generic),
                controller_toggle(t!("Controller vibration (rumble)"), cal.rumble_enabled, |c| &mut c.calibration.rumble_enabled),
                controller_toggle(t!("Enable gyroscope & motion sensors"), cal.gyro_enabled, |c| &mut c.calibration.gyro_enabled),
                controller_toggle(t!("Use Nintendo button layout"), cal.nintendo_layout, |c| &mut c.calibration.nintendo_layout),
                deadzone(t!("Left stick deadzone"), cal.left_deadzone, |c| &mut c.left_deadzone),
                deadzone(t!("Right stick deadzone"), cal.right_deadzone, |c| &mut c.right_deadzone),
                deadzone(t!("Trigger threshold"), cal.trigger_threshold, |c| &mut c.trigger_threshold),
                Row::Button {
                    icon: icons::RESTORE, label: t!("Reset calibration").into(), danger: false, checked: false,
                    act: controller_change(|c| c.calibration = client::PadCalibration::default()),
                },
                Row::Button {
                    icon: icons::GEAR, label: t!("Advanced controller settings (desktop)").into(), danger: false, checked: false,
                    act: act(|bp, f, ctx| {
                        bp.leave(ctx);
                        f.actions.push(Action::Navigate { page: "settings".into(), arg: Some("controller".into()) });
                    }),
                },
            ]);
            if cal.gyro_enabled {
                let step = (cal.gyro_sensitivity * 4.0).round().clamp(1.0, 16.0) as u8;
                let next = if step == 16 { 1 } else { step + 1 };
                out.insert(out.len() - 2, choice(t!("Gyro sensitivity"), "", format!("{:.2}×", cal.gyro_sensitivity),
                    controller_change(move |c| c.calibration.gyro_sensitivity = f32::from(next) / 4.0)));
            }
            for pad in devices.controllers {
                let value = pad.battery_percent.map(|n| format!("{n}%")).unwrap_or_else(|| pad.backend.to_string());
                out.push(Row::Info { label: pad.name.clone(), value });
                if pad.has_rumble && cal.rumble_enabled {
                    out.push(Row::Button {
                        icon: icons::CONTROLLER, label: t!("Test vibration: {controller}", controller = pad.name),
                        danger: false, checked: false,
                        act: act(move |_, _, _| client::trigger_test_rumble(&pad.id)),
                    });
                }
            }
            out
        }
        5 => {
            let corner = match flags.fps_counter {
                FpsCorner::Off => t!("Off"),
                FpsCorner::TopLeft => t!("Top left"),
                FpsCorner::TopRight => t!("Top right"),
                FpsCorner::BottomLeft => t!("Bottom left"),
                FpsCorner::BottomRight => t!("Bottom right"),
            };
            vec![
                toggle(t!("Show the overlay in games"), t!("Open in supported games with Shift+Tab."), flags.overlay_enabled, flag(|x| &mut x.overlay_enabled)),
                choice(t!("FPS counter"), t!("Shows how many frames per second the game draws, in a corner of the screen."), corner.to_string(), act(|_, f, _| {
                    let flags = f.app.shared_flags();
                    let mut l = flags.lock().unwrap();
                    l.fps_counter = next_in(&CORNERS, l.fps_counter);
                })),
            ]
        }
        6 => {
            let themes = bp.settings.themes.get_or_insert_with(|| {
                let state = f.app.themes.lock().unwrap();
                std::iter::once((None, t!("Default").into()))
                    .chain(state.manager.installed.iter().map(|theme| {
                        let meta = &theme.manifest.meta;
                        (Some(meta.id.clone()), meta.name.clone())
                    }))
                    .collect()
            });
            themes.iter().map(|(id, name)| {
                let selected = id.clone();
                Row::Button {
                    icon: icons::PAINT,
                    label: name.clone(),
                    danger: false,
                    checked: *id == f.app.prefs.active_theme_id,
                    act: act(move |_, f, _| f.actions.push(Action::ActivateTheme(selected))),
                }
            }).collect()
        }
        7 => {
            let me = f.app.state.me().map(|m| m.state).unwrap_or(PersonaState::Online);
            let armed = f.now < bp.settings.confirm_until;
            vec![
                Row::Info { label: t!("Profile name").to_string(), value: f.app.state.persona_name.clone() },
                choice(t!("Status"), "", i18n::tr_dyn(me.label()), act(move |_, f, _| f.app.client.send(Command::SetPersonaState(next_in(&STATUSES, me))))),
                toggle(t!("Ask which account to use"), t!("Show the account picker at startup instead of signing in automatically."), flags.ask_account, flag(|x| &mut x.ask_account)),
                toggle(t!("Close to tray"), t!("Keep SteamInRust running in the system tray when you close the window."), flags.close_to_tray, flag(|x| &mut x.close_to_tray)),
                Row::Button {
                    icon: icons::SIGN_OUT,
                    label: if armed { t!("Select again to sign out").to_string() } else { t!("Sign out of account").to_string() },
                    danger: true,
                    checked: false,
                    act: act(move |bp, f, ctx| {
                        if armed {
                            bp.settings.confirm_until = 0.0;
                            bp.leave(ctx);
                            if !f.app.demo {
                                f.app.client.send(Command::LogOut);
                            }
                        } else {
                            bp.settings.confirm_until = f.now + 4.0;
                        }
                    }),
                },
            ]
        }
        #[cfg(target_os = "linux")]
        8 => {
            let current = client::proton_override();
            let builds = client::proton_builds();
            let mut out = vec![Row::Button {
                icon: icons::WINDOWS, label: t!("Automatic").into(), danger: false, checked: current.is_none(),
                act: act(|_, _, _| client::set_proton_override(None)),
            }];
            if builds.is_empty() {
                out.push(Row::Info { label: t!("No installed Proton versions found.").into(), value: String::new() });
            } else if let Some(saved) = current.as_deref().filter(|saved| !builds.iter().any(|(name, _)| saved.eq_ignore_ascii_case(name))) {
                out.push(Row::Info { label: t!("Saved Proton is unavailable").into(), value: saved.to_string() });
            }
            out.extend(builds.into_iter().map(|(name, label)| {
                let checked = current.as_deref().is_some_and(|saved| saved.eq_ignore_ascii_case(&name));
                Row::Button {
                    icon: icons::WINDOWS, label, danger: false, checked,
                    act: act(move |_, _, _| client::set_proton_override(Some(name))),
                }
            }));
            out
        }
        _ => {
            let lang = i18n::language();
            let langs = i18n::Lang::ALL;
            let other = langs[(langs.iter().position(|l| *l == lang).unwrap_or(0) + 1) % langs.len()];
            vec![
                choice(t!("Interface size"), "", format!("{}%", (f.app.prefs.bigpicture_scale * 100.0).round() as u32), act(|_, f, _| {
                    let at = SIZES.iter().position(|x| (*x - f.app.prefs.bigpicture_scale).abs() < 0.01).unwrap_or(0);
                    f.app.prefs.bigpicture_scale = SIZES[(at + 1) % SIZES.len()];
                })),
                toggle(t!("Fullscreen"), t!("Press F11 to switch at any time in Big Picture."), !bp.is_windowed(),
                    act(|bp, _, ctx| bp.toggle_fullscreen(ctx))),
                toggle(t!("Start SteamInRust in Big Picture"), "", f.app.prefs.bigpicture_start, act(|_, f, _| f.app.prefs.bigpicture_start = !f.app.prefs.bigpicture_start)),
                choice(t!("Language"), "", lang.native_name().to_string(), act(move |_, f, _| f.actions.push(Action::SetLanguage(other.code().into())))),
                Row::Button {
                    icon: icons::GEAR,
                    label: t!("All settings (leaves Big Picture)").to_string(),
                    danger: false,
                    checked: false,
                    act: act(|bp, f, ctx| {
                        bp.leave(ctx);
                        f.actions.push(Action::Navigate { page: "settings".into(), arg: None });
                    }),
                },
            ]
        }
    }
}

fn nav_id(i: usize) -> Id {
    Id::unique(("bp-set-nav", i))
}

/// The settings page: sections on the left, their rows on the right.
pub fn page(bp: &mut BigPicture, ui: &mut Ui, f: &mut Frame) -> f32 {
    let s = f.s;
    let left = f.view.left() + f.pad;
    let top = f.view.top() + 12.0 * s;
    let p = ui.painter().clone();
    p.text(pos2(left, top), Align2::LEFT_TOP, t!("Settings"), w::font_bold(40.0 * s), f.t.text);
    let body_top = top + 76.0 * s;

    // Between the two columns: left from a row goes to its own section (not
    // the one beside it), up or down past the last row stays put, and right
    // from a section goes to its first row.
    let list = sections();
    let cur = bp.focus.current;
    let was = bp.settings.last_focus;
    let is_nav = |id: Option<Id>| (0..list.len()).any(|i| id == Some(nav_id(i)));
    let is_tab = |id: Option<Id>| super::TABS.iter().any(|tab| id == Some(super::tab_id(*tab)));
    let from_row = was.is_some() && !is_nav(was) && !is_tab(was);
    let dir = bp.focus.last_dir;
    if bp.focus.moved && is_nav(cur) && from_row {
        bp.focus.current = if dir == Some(Dir::Left) { Some(nav_id(bp.settings.section)) } else { was };
    }
    // The columns aren't the same height: nothing may be right beside you.
    if !bp.focus.moved && dir == Some(Dir::Left) && from_row && cur == was {
        bp.focus.current = Some(nav_id(bp.settings.section));
    }
    let into_rows = dir == Some(Dir::Right) && is_nav(was) && !is_tab(cur) && (bp.focus.moved || cur == was);
    bp.settings.last_focus = bp.focus.current;

    let nav_w = 430.0 * s;
    let nav_h = 72.0 * s;
    let mut enter_rows = false;
    for (i, (icon, label)) in list.iter().enumerate() {
        let r = Rect::from_min_size(pos2(left, body_top + i as f32 * (nav_h + 6.0 * s)), vec2(nav_w, nav_h));
        let hit = bp.focus.item(ui, nav_id(i), r, ui.clip_rect(), Place::Page);
        if hit.focused && bp.settings.section != i {
            bp.settings.section = i;
            bp.settings.confirm_until = 0.0;
            let sc = bp.scroller("rows");
            sc.pos = 0.0;
            sc.target = 0.0;
        }
        let on = bp.settings.section == i;
        let fill = if on { f.t.accent.gamma_multiply(0.32 + 0.2 * hit.glow) } else { w::mix(Color32::TRANSPARENT, Color32::from_white_alpha(22), hit.glow) };
        p.rect_filled(r, paint::radius(14.0 * s), fill);
        let c = if on { f.t.text } else { f.t.text_dim };
        p.text(pos2(r.left() + 38.0 * s, r.center().y), Align2::CENTER_CENTER, icon, w::font(28.0 * s), c);
        paint::line(&p, pos2(r.left() + 72.0 * s, r.center().y), Align2::LEFT_CENTER, label, w::font_semibold(23.0 * s), c, r.width() - 86.0 * s);
        if hit.focused {
            bp.focus.ring_around(r, 14.0 * s);
        }
        enter_rows |= hit.activated;
    }
    bp.preferred = Some(nav_id(bp.settings.section));

    // The rows, scrolling on their own.
    let x = left + nav_w + 36.0 * s;
    let width = (f.view.right() - f.pad - x).min(1120.0 * s);
    let area = Rect::from_min_max(pos2(x - 12.0 * s, body_top - 8.0 * s), pos2(x + width + 12.0 * s, f.view.bottom() - 6.0 * s));
    let list_rows = rows(bp, f);
    let section = bp.settings.section;
    let pos = bp.scroller("rows").pos;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
    child.set_clip_rect(area.intersect(ui.clip_rect()));
    let q = child.painter().clone();
    let mut y = body_top - pos;
    let mut picked: Option<Act> = None;
    let mut first_row = None;
    let mut reveal = None;
    let row_top = y;
    for (i, row) in list_rows.into_iter().enumerate() {
        let has_note = matches!(&row, Row::Toggle { note, .. } | Row::Choice { note, .. } if !note.is_empty());
        let h = if has_note { 104.0 } else { 80.0 } * s;
        let r = Rect::from_min_size(pos2(x, y), vec2(width, h));
        y += h + 10.0 * s;
        if let Row::Info { label, value } = &row {
            q.rect_filled(r, paint::radius(14.0 * s), f.t.surface.gamma_multiply(0.55));
            paint::line(&q, pos2(r.left() + 26.0 * s, r.center().y), Align2::LEFT_CENTER, label, w::font_semibold(23.0 * s), f.t.text_dim, r.width() * 0.6);
            paint::line(&q, pos2(r.right() - 26.0 * s, r.center().y), Align2::RIGHT_CENTER, value, w::font_medium(23.0 * s), f.t.text, r.width() * 0.36);
            continue;
        }
        let id = Id::unique(("bp-set-row", section, i));
        first_row.get_or_insert(id);
        let hit = bp.focus.item(&child, id, r, area, Place::Bar);
        if hit.focused && bp.focus.moved {
            reveal = Some((r.top() - row_top, r.bottom() - row_top));
        }
        let shown = paint::grow(r, 1.0 + 0.012 * hit.glow);
        q.rect_filled(shown, paint::radius(14.0 * s), w::mix(f.t.surface.gamma_multiply(0.85), f.t.surface_hover, hit.glow));
        if hit.focused {
            bp.focus.ring_around(shown, 14.0 * s);
        }
        let text_x = shown.left() + 26.0 * s;
        let right = shown.right() - 26.0 * s;
        let title_at = |note: &str| if note.is_empty() { shown.center().y } else { shown.top() + 34.0 * s };
        let draw_note = |note: &str, room: f32| {
            if !note.is_empty() {
                paint::line(&q, pos2(text_x, shown.top() + 70.0 * s), Align2::LEFT_CENTER, note, w::font(18.0 * s), f.t.text_dim, room);
            }
        };
        let action = match row {
            Row::Toggle { label, note, on, act } => {
                let k = child.ctx().animate_bool_with_time(id.with("switch"), on, 0.14);
                let track = Rect::from_center_size(pos2(right - 36.0 * s, shown.center().y), vec2(72.0 * s, 38.0 * s));
                q.rect_filled(track, paint::radius(19.0 * s), w::mix(f.t.raised, f.t.accent, k));
                q.circle_filled(pos2(track.left() + 19.0 * s + k * 34.0 * s, track.center().y), 14.0 * s, Color32::WHITE);
                let room = track.left() - text_x - 20.0 * s;
                paint::line(&q, pos2(text_x, title_at(&note)), Align2::LEFT_CENTER, &label, w::font_semibold(23.0 * s), f.t.text, room);
                draw_note(&note, room);
                act
            }
            Row::Choice { label, note, value, act } => {
                let vr = paint::line(&q, pos2(right - 26.0 * s, shown.center().y), Align2::RIGHT_CENTER, &value, w::font_medium(23.0 * s), f.t.accent, width * 0.42);
                q.text(pos2(right - 8.0 * s, shown.center().y), Align2::CENTER_CENTER, icons::CARET_RIGHT, w::font(20.0 * s), f.t.accent);
                let room = vr.left() - text_x - 20.0 * s;
                paint::line(&q, pos2(text_x, title_at(&note)), Align2::LEFT_CENTER, &label, w::font_semibold(23.0 * s), f.t.text, room);
                draw_note(&note, room);
                act
            }
            Row::Button { icon, label, danger, checked, act } => {
                let c = if danger { f.t.danger } else { f.t.text };
                q.text(pos2(text_x + 14.0 * s, shown.center().y), Align2::CENTER_CENTER, icon, w::font(26.0 * s), c);
                paint::line(&q, pos2(text_x + 50.0 * s, shown.center().y), Align2::LEFT_CENTER, &label, w::font_semibold(23.0 * s), c, width - 150.0 * s);
                if checked {
                    q.text(pos2(right - 14.0 * s, shown.center().y), Align2::CENTER_CENTER, icons::CHECK, w::font(26.0 * s), f.t.accent);
                }
                act
            }
            Row::Info { .. } => continue,
        };
        if hit.activated {
            picked = Some(action);
        }
    }
    let height = y - row_top;
    let view = area.height() - 8.0 * s;
    let wheel = if ui.rect_contains_pointer(area) { ui.input(|i| i.smooth_scroll_delta.y) } else { 0.0 };
    let sc = bp.scroller("rows");
    sc.target -= wheel;
    if let Some((a, b)) = reveal {
        sc.reveal(a, b, view, 20.0 * s);
    }
    sc.clamp(height, view);
    sc.step(ui);
    if enter_rows || into_rows {
        if let Some(id) = first_row {
            bp.focus.current = Some(id);
            bp.settings.last_focus = Some(id);
            let sc = bp.scroller("rows");
            sc.target = 0.0;
        }
    }
    if let Some(a) = picked {
        let ctx = ui.ctx().clone();
        a(bp, f, &ctx);
    }
    f.view.height()
}
