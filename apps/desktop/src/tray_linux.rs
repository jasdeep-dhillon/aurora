//! Linux tray icon: a StatusNotifierItem over D-Bus (KDE Plasma, GNOME with
//! the AppIndicator extension, Cinnamon, XFCE, waybar…) with the desktop's own
//! menu. No GTK loop needed, unlike tray-icon.
//!
//! Capture activation tokens during ksni callbacks and send them to the UI thread.

use crate::tray::{TrayCmd, TrayEvent};
use i18n::t;
use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{MenuItem, OfflineReason};
use model::PersonaState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

/// What the menu shows (updated only when it changes).
#[derive(Clone, PartialEq, Default)]
pub struct MenuInfo {
    pub signed_in: bool,
    pub name: String,
    pub state: PersonaState,
    pub online: usize,
    pub download: Option<String>,
    pub language: String,
}

pub struct LinuxTray {
    handle: Handle<Sni>,
    online: Arc<AtomicBool>,
    shown: MenuInfo,
}

struct Sni {
    tx: mpsc::Sender<TrayEvent>,
    ctx: egui::Context,
    info: MenuInfo,
    icons: Vec<ksni::Icon>,
    wayland: bool,
    online: Arc<AtomicBool>,
}

const STATES: [PersonaState; 4] = [PersonaState::Online, PersonaState::Away, PersonaState::Busy, PersonaState::Invisible];

impl LinuxTray {
    /// `None` when no tray host is running (e.g. GNOME without the
    /// AppIndicator extension): closing then quits as before.
    pub fn spawn(tx: mpsc::Sender<TrayEvent>, ctx: egui::Context, wayland: bool) -> Option<Self> {
        let online = Arc::new(AtomicBool::new(true));
        let icons = [22, 32, 48, 64].into_iter().map(icon).collect();
        let sni = Sni { tx, ctx, info: MenuInfo::default(), icons, wayland, online: online.clone() };
        match sni.spawn() {
            Ok(handle) => Some(Self { handle, online, shown: MenuInfo::default() }),
            Err(e) => {
                tracing::info!("no tray icon: {e}");
                None
            }
        }
    }

    /// A tray host shows the icon right now.
    pub fn available(&self) -> bool {
        self.online.load(Ordering::Relaxed) && !self.handle.is_closed()
    }

    pub fn set_info(&mut self, info: MenuInfo) {
        if info != self.shown {
            self.shown = info.clone();
            self.handle.update(|s| s.info = info);
        }
    }
}

impl Sni {
    fn pick(&mut self, command: TrayCmd, raise: bool) {
        let activation_token = if raise && self.wayland { click_activation_token() } else { None };
        let _ = self.tx.send(TrayEvent { command, activation_token });
        self.ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}

/// GNOME sends the token before the click. KDE may deliver it shortly after.
fn click_activation_token() -> Option<String> {
    for _ in 0..5 {
        if let Some(token) = ksni::current_xdg_activation_token() {
            return Some(token);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    tracing::debug!("tray click came without an activation token");
    None
}

/// Labels treat `_` as an access-key marker.
fn label(s: impl AsRef<str>) -> String {
    s.as_ref().replace('_', "__")
}

fn item(text: impl AsRef<str>, cmd: TrayCmd, raise: bool) -> MenuItem<Sni> {
    StandardItem { label: label(text), activate: Box::new(move |s: &mut Sni| s.pick(cmd, raise)), ..Default::default() }.into()
}

impl ksni::Tray for Sni {
    fn id(&self) -> String {
        "aurora".into()
    }

    fn title(&self) -> String {
        i18n::BRAND.into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let mut description = String::new();
        if self.info.signed_in {
            description = self.info.name.clone();
        }
        if let Some(d) = &self.info.download {
            description = format!("{description}\n{} {d}", t!("Downloads"));
        }
        ksni::ToolTip { title: i18n::BRAND.into(), description: description.trim().into(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.pick(TrayCmd::Show, true);
    }

    fn watcher_online(&self) {
        self.online.store(true, Ordering::Relaxed);
        self.ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    fn watcher_offline(&self, _reason: OfflineReason) -> bool {
        // Keep waiting: the shell may restart (or the extension get enabled).
        self.online.store(false, Ordering::Relaxed);
        self.ctx.request_repaint_of(egui::ViewportId::ROOT);
        true
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let d = &self.info;
        let mut menu = vec![item(t!("Open {brand}", brand = i18n::BRAND), TrayCmd::Show, true)];
        if d.signed_in {
            let options = STATES.iter().map(|s| RadioItem { label: label(i18n::tr_dyn(s.label())), ..Default::default() }).collect();
            menu.push(
                SubMenu {
                    label: label(format!("{} — {}", d.name, i18n::tr_dyn(d.state.label()))),
                    submenu: vec![
                        RadioGroup { selected: STATES.iter().position(|s| *s == d.state).unwrap_or(usize::MAX), select: Box::new(|s: &mut Sni, i| s.pick(TrayCmd::Status(STATES[i]), false)), options }
                            .into(),
                    ],
                    ..Default::default()
                }
                .into(),
            );
        }
        menu.push(MenuItem::Separator);
        let with = |text: &str, extra: Option<String>| match extra {
            Some(e) => format!("{text}  ({e})"),
            None => text.to_string(),
        };
        let online = (d.online > 0).then(|| t!("{n} online", n = d.online));
        menu.extend([
            item(t!("Store"), TrayCmd::Page("store"), true),
            item(t!("Library"), TrayCmd::Page("library"), true),
            item(t!("Community"), TrayCmd::Page("community_home"), true),
            item(with(t!("Friends"), online), TrayCmd::Friends, false),
            item(t!("Profile"), TrayCmd::Page("profile"), true),
            item(with(t!("Downloads"), d.download.clone()), TrayCmd::Page("downloads"), true),
            item(t!("Settings"), TrayCmd::Page("settings"), true),
            MenuItem::Separator,
            item(t!("Exit Aurora"), TrayCmd::Exit, true),
        ]);
        menu
    }
}

/// The app icon as ARGB32 (network byte order).
fn icon(size: u32) -> ksni::Icon {
    let mut data = ui::brand::icon_rgba(size);
    for px in data.chunks_exact_mut(4) {
        px.rotate_right(1); // RGBA → ARGB
    }
    ksni::Icon { width: size as i32, height: size as i32, data }
}
