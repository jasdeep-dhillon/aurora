//! Shared modals and progress panels for moving, backing up, and restoring games.

use client::Command;
use egui::{Align, Layout, Ui, vec2};
use extensions::{Action, Ctx};
use i18n::t;
use local::manage::{BackupInfo, available_space};
use model::{AppId, InstalledGame, art};
use std::path::{Path, PathBuf};
use ui::{Theme, icons, widgets as w};

/// Modal state for moving, backing up, or restoring a game.
pub(super) enum StorageDialog {
    Move(MoveDialog),
    Backup(BackupDialog),
    Restore(RestoreDialog),
}

impl StorageDialog {
    pub(super) fn show(&mut self, ui: &Ui, t: &Theme, ctx: &mut Ctx, salt: &str) -> bool {
        match self {
            StorageDialog::Move(d) => d.show(ui, t, ctx, salt),
            StorageDialog::Backup(d) => d.show(ui, t, ctx, salt),
            StorageDialog::Restore(d) => d.show(ui, t, ctx, salt),
        }
    }
}

pub(super) struct MoveDialog {
    game: InstalledGame,
    name: String,
    libraries: Vec<PathBuf>,
    dest: Option<PathBuf>,
}

impl MoveDialog {
    pub(super) fn new(game: InstalledGame, name: String, ctx: &Ctx) -> Self {
        let libraries: Vec<PathBuf> = ctx
            .local
            .libraries
            .iter()
            .filter(|l| !same_lib(l, &game.library))
            .cloned()
            .collect();
        let dest = libraries
            .iter()
            .max_by_key(|l| available_space(l).unwrap_or(0))
            .cloned();
        Self { game, name, libraries, dest }
    }

    fn show(&mut self, ui: &Ui, t: &Theme, ctx: &mut Ctx, salt: &str) -> bool {
        let app = self.game.app_id;
        let running = ctx.state.playing == Some(app);
        let downloading = is_downloading(ctx, app);
        let busy = ctx.state.moving.contains_key(&app) || ctx.state.backing_up.contains_key(&app);
        let need = self.game.size_on_disk;
        let mut go = false;
        let mut close = false;

        w::modal(ui.ctx(), egui::Id::unique(("move-game", salt, app))).show(ui.ctx(), |ui| {
            ui.set_width(520.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(140.0, 65.0), egui::Sense::hover());
                w::image_or_skeleton(ui, t, r, &art::header(app), t.corner_small());
                ui.vertical(|ui| {
                    ui.label(w::text(t!("Move install folder"), 13.0, t.text_dim));
                    ui.label(w::text_bold(&self.name, 19.0, t.text));
                });
            });
            ui.add_space(10.0);
            ui.label(w::text(t!("Current folder:"), 12.5, t.text_dim));
            ui.add(egui::Label::new(w::text(self.game.install_dir.display().to_string(), 12.5, t.text).monospace()).truncate());
            ui.add_space(10.0);
            ui.label(w::text_medium(t!("Destination library folder"), 12.5, t.text_dim));
            if self.libraries.is_empty() {
                ui.label(w::text(t!("No other library folders configured yet. Add one below."), 13.0, t.text_faint));
            }
            let mut picked_dest = self.dest.clone().unwrap_or_default();
            for lib in &self.libraries {
                let free = available_space(lib);
                let label = format!(
                    "{}   ·   {}",
                    lib.display(),
                    t!("{size} free", size = free.map(ui::bytes).unwrap_or_else(|| "?".into()))
                );
                if w::radio_value(ui, t, &mut picked_dest, lib.clone(), w::text(label, 13.5, t.text)).changed() {
                    self.dest = Some(picked_dest.clone());
                }
            }
            if w::button(ui, t, Some(icons::FOLDER), t!("Choose another folder…"), w::Kind::Ghost, w::Size::Sm).clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    if same_lib(&p, &self.game.library) {
                        ctx.toast(t!("Choose a different library folder."));
                    } else {
                        if let Err(e) = local::add_library(&p) {
                            ctx.toast(format!("{}: {e}", p.display()));
                        } else {
                            ctx.act(Action::ReloadLocal);
                        }
                        if !self.libraries.iter().any(|l| same_lib(l, &p)) {
                            self.libraries.push(p.clone());
                        }
                        self.dest = Some(p);
                    }
                }
            }
            ui.add_space(8.0);
            if need > 0 {
                ui.label(w::text(t!("Space required: {size}", size = ui::bytes(need)), 13.0, t.text));
            }
            let enough_space = self
                .dest
                .as_deref()
                .and_then(available_space)
                .is_none_or(|avail| avail >= need);
            if !enough_space {
                ui.label(w::text(format!("{}  {}", icons::WARNING, t!("Not enough free space on this drive.")), 13.0, t.danger));
            }
            if running {
                ui.label(w::text(format!("{}  {}", icons::WARNING, t!("Close the game first.")), 13.0, t.away));
            } else if downloading {
                ui.label(w::text(format!("{}  {}", icons::WARNING, t!("Wait for the download to finish first.")), 13.0, t.away));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let can = self.dest.is_some() && enough_space && !running && !downloading && !busy;
                if ui.add_enabled_ui(can, |ui| w::button(ui, t, Some(icons::DRIVES), t!("Move"), w::Kind::Primary, w::Size::Md)).inner.clicked() {
                    go = true;
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Secondary, w::Size::Md).clicked() {
                    close = true;
                }
            });
        });

        if go && let Some(dest_library) = self.dest.clone() {
            ctx.send(Command::MoveGame { game: self.game.clone(), dest_library });
            return false;
        }
        !close
    }
}

pub(super) struct BackupDialog {
    game: InstalledGame,
    name: String,
    folder: Option<PathBuf>,
    target: Option<PathBuf>,
}

impl BackupDialog {
    pub(super) fn new(game: InstalledGame, name: String) -> Self {
        Self { game, name, folder: None, target: None }
    }

    fn compute_target(base: &Path, name: &str, app_id: AppId) -> PathBuf {
        let sub = safe_backup_folder_name(name, app_id);
        let candidate = base.join(&sub);
        if !candidate.exists() {
            return candidate;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        base.join(format!("{sub}-{stamp}"))
    }

    fn show(&mut self, ui: &Ui, t: &Theme, ctx: &mut Ctx, salt: &str) -> bool {
        let app = self.game.app_id;
        let running = ctx.state.playing == Some(app);
        let downloading = is_downloading(ctx, app);
        let busy = ctx.state.moving.contains_key(&app) || ctx.state.backing_up.contains_key(&app);
        let need = self.game.size_on_disk;
        let mut go = false;
        let mut close = false;

        w::modal(ui.ctx(), egui::Id::unique(("backup-game", salt, app))).show(ui.ctx(), |ui| {
            ui.set_width(520.0);
            ui.label(w::text(icons::PACKAGE, 26.0, t.accent));
            ui.label(w::text_bold(t!("Back up {name}", name = self.name), 19.0, t.text));
            ui.add_space(4.0);
            ui.add(egui::Label::new(w::text(t!("Creates a verified copy of the game files and manifest so you can restore it later without re-downloading."), 13.0, t.text_dim)).wrap());
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if w::button(ui, t, Some(icons::FOLDER), t!("Choose backup folder…"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.target = Some(Self::compute_target(&p, &self.name, app));
                        self.folder = Some(p);
                    }
                }
                if let Some(f) = &self.folder {
                    ui.add(egui::Label::new(w::text(f.display().to_string(), 12.5, t.text).monospace()).truncate());
                } else {
                    ui.label(w::text(t!("No folder chosen"), 12.5, t.text_faint));
                }
            });
            if let Some(target) = &self.target {
                ui.add_space(4.0);
                ui.add(egui::Label::new(w::text(t!("Backup will be saved to: {path}", path = target.display()), 12.0, t.text_dim)).wrap());
            }
            ui.add_space(8.0);
            if need > 0 {
                ui.label(w::text(t!("Space required: {size}", size = ui::bytes(need)), 13.0, t.text));
            }
            let free = self.folder.as_deref().and_then(available_space);
            if let Some(avail) = free {
                ui.label(w::text(t!("Available space: {size}", size = ui::bytes(avail)), 12.5, t.text_dim));
            }
            let enough_space = free.is_none_or(|avail| avail >= need);
            if !enough_space {
                ui.label(w::text(format!("{}  {}", icons::WARNING, t!("Not enough free space on this drive.")), 13.0, t.danger));
            }
            if running {
                ui.label(w::text(format!("{}  {}", icons::WARNING, t!("Close the game first.")), 13.0, t.away));
            } else if downloading {
                ui.label(w::text(format!("{}  {}", icons::WARNING, t!("Wait for the download to finish first.")), 13.0, t.away));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let can = self.target.is_some() && enough_space && !running && !downloading && !busy;
                if ui.add_enabled_ui(can, |ui| w::button(ui, t, Some(icons::PACKAGE), t!("Back up"), w::Kind::Primary, w::Size::Md)).inner.clicked() {
                    go = true;
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Secondary, w::Size::Md).clicked() {
                    close = true;
                }
            });
        });

        if go && let Some(dest_dir) = self.target.clone() {
            ctx.send(Command::BackupGame { game: self.game.clone(), dest_dir });
            return false;
        }
        !close
    }
}

pub(super) struct RestoreDialog {
    backup_dir: Option<PathBuf>,
    inspected: Option<Result<BackupInfo, String>>,
    libraries: Vec<PathBuf>,
    dest_library: Option<PathBuf>,
}

impl RestoreDialog {
    pub(super) fn new(ctx: &Ctx) -> Self {
        let libraries = ctx.local.libraries.clone();
        let dest_library = libraries
            .iter()
            .max_by_key(|l| available_space(l).unwrap_or(0))
            .cloned();
        Self { backup_dir: None, inspected: None, libraries, dest_library }
    }

    fn show(&mut self, ui: &Ui, t: &Theme, ctx: &mut Ctx, salt: &str) -> bool {
        let restoring = ctx.state.restoring.is_some();
        let mut go = false;
        let mut close = false;

        w::modal(ui.ctx(), egui::Id::unique(("restore-backup", salt))).show(ui.ctx(), |ui| {
            ui.set_width(540.0);
            ui.label(w::text(icons::RESTORE, 26.0, t.accent));
            ui.label(w::text_bold(t!("Restore game backup"), 19.0, t.text));
            ui.add_space(4.0);
            ui.add(egui::Label::new(w::text(t!("Choose an Aurora backup folder or a Steam backup folder containing appmanifest and game files."), 13.0, t.text_dim)).wrap());
            ui.add_space(10.0);

            ui.label(w::text_medium(t!("Backup folder"), 12.5, t.text_dim));
            ui.horizontal(|ui| {
                if w::button(ui, t, Some(icons::FOLDER), t!("Choose backup folder…"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.inspected = Some(local::manage::inspect_backup(&p));
                        self.backup_dir = Some(p);
                    }
                }
                if let Some(p) = &self.backup_dir {
                    ui.add(egui::Label::new(w::text(p.display().to_string(), 12.5, t.text).monospace()).truncate());
                } else {
                    ui.label(w::text(t!("No folder chosen"), 12.5, t.text_faint));
                }
            });

            if let Some(res) = &self.inspected {
                ui.add_space(8.0);
                match res {
                    Ok(info) => {
                        w::panel(t).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(w::text_semibold(&info.name, 14.5, t.text));
                                if info.is_aurora {
                                    w::pill(ui, t!("Aurora verified"), t.play, t.play.gamma_multiply(0.15));
                                } else {
                                    w::pill(ui, t!("Steam backup"), t.accent, t.accent.gamma_multiply(0.15));
                                }
                            });
                            let detail = if info.total_bytes > 0 {
                                format!("AppID {}  ·  {}  ·  {}", info.app_id, info.install_dir, ui::bytes(info.total_bytes))
                            } else {
                                format!("AppID {}  ·  {}", info.app_id, info.install_dir)
                            };
                            ui.label(w::text(detail, 12.5, t.text_dim));
                        });
                    }
                    Err(e) => {
                        ui.add(egui::Label::new(w::text(format!("{}  {}", icons::WARNING, i18n::tr_dyn(e)), 13.0, t.danger)).wrap());
                    }
                }
            }

            ui.add_space(10.0);
            ui.label(w::text_medium(t!("Restore to library folder"), 12.5, t.text_dim));
            if self.libraries.is_empty() {
                ui.label(w::text(t!("No library folders configured yet. Add one below."), 13.0, t.text_faint));
            }
            let mut picked_lib = self.dest_library.clone().unwrap_or_default();
            for lib in &self.libraries {
                let free = available_space(lib);
                let label = format!(
                    "{}   ·   {}",
                    lib.display(),
                    t!("{size} free", size = free.map(ui::bytes).unwrap_or_else(|| "?".into()))
                );
                if w::radio_value(ui, t, &mut picked_lib, lib.clone(), w::text(label, 13.5, t.text)).changed() {
                    self.dest_library = Some(picked_lib.clone());
                }
            }
            if w::button(ui, t, Some(icons::FOLDER), t!("Choose another folder…"), w::Kind::Ghost, w::Size::Sm).clicked() {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    if let Err(e) = local::add_library(&p) {
                        ctx.toast(format!("{}: {e}", p.display()));
                    } else {
                        ctx.act(Action::ReloadLocal);
                    }
                    if !self.libraries.iter().any(|l| same_lib(l, &p)) {
                        self.libraries.push(p.clone());
                    }
                    self.dest_library = Some(p);
                }
            }

            let info_ok = self.inspected.as_ref().and_then(|r| r.as_ref().ok());
            let already_installed = info_ok.is_some_and(|info| {
                self.dest_library.as_ref().is_some_and(|lib| {
                    ctx.local.games.iter().any(|g| g.app_id == info.app_id && same_lib(&g.library, lib))
                })
            });
            let need = info_ok.map_or(0, |i| i.total_bytes);
            let enough_space = self
                .dest_library
                .as_deref()
                .and_then(available_space)
                .is_none_or(|avail| avail >= need);
            if !enough_space {
                ui.label(w::text(format!("{}  {}", icons::WARNING, t!("Not enough free space on this drive.")), 13.0, t.danger));
            }
            if already_installed {
                ui.label(w::text(format!("{}  {}", icons::WARNING, t!("This game is already installed in this library folder.")), 13.0, t.away));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let can = info_ok.is_some() && self.dest_library.is_some() && enough_space && !already_installed && !restoring;
                if ui.add_enabled_ui(can, |ui| w::button(ui, t, Some(icons::RESTORE), t!("Restore"), w::Kind::Primary, w::Size::Md)).inner.clicked() {
                    go = true;
                }
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Secondary, w::Size::Md).clicked() {
                    close = true;
                }
            });
        });

        if go && let (Some(backup_dir), Some(dest_library)) = (self.backup_dir.clone(), self.dest_library.clone()) {
            ctx.send(Command::RestoreGame { backup_dir, dest_library });
            return false;
        }
        !close
    }
}

pub(super) fn move_progress_panel(ui: &mut Ui, t: &Theme, ctx: &Ctx, app_id: AppId, p: &client::state::TransferProgress) {
    let pct = (p.fraction() * 100.0).round() as u32;
    transfer_progress_panel(ui, t, &t!("Moving install folder… {pct}%", pct = pct), p.done_bytes, p.total_bytes, p.fraction(), || {
        ctx.send(Command::CancelMove { app_id });
    });
}

pub(super) fn backup_progress_panel(ui: &mut Ui, t: &Theme, ctx: &Ctx, app_id: AppId, p: &client::state::TransferProgress) {
    let pct = (p.fraction() * 100.0).round() as u32;
    transfer_progress_panel(ui, t, &t!("Backing up game files… {pct}%", pct = pct), p.done_bytes, p.total_bytes, p.fraction(), || {
        ctx.send(Command::CancelBackup { app_id });
    });
}

fn transfer_progress_panel(ui: &mut Ui, t: &Theme, label: &str, done: u64, total: u64, frac: f32, on_cancel: impl FnOnce()) {
    w::panel(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(w::text_semibold(label, 14.0, t.accent));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::button(ui, t, Some(icons::X), t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                    on_cancel();
                }
                ui.label(w::text(format!("{} / {}", ui::bytes(done), ui::bytes(total)), 12.5, t.text_dim));
            });
        });
        w::progress(ui, t, frac, ui.available_width());
    });
}

pub(super) fn restore_progress_card(ui: &mut Ui, t: &Theme, ctx: &Ctx, p: &client::state::RestoreProgress) {
    let pct = (p.fraction() * 100.0).round() as u32;
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(w::text_semibold(t!("Restoring {name}… {pct}%", name = p.name, pct = pct), 14.5, t.accent));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::button(ui, t, Some(icons::X), t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                    ctx.send(Command::CancelRestore);
                }
                ui.label(w::text(format!("{} / {}", ui::bytes(p.done_bytes), ui::bytes(p.total_bytes)), 12.5, t.text_dim));
            });
        });
        ui.add_space(4.0);
        w::progress(ui, t, p.fraction(), ui.available_width());
    });
}

fn is_downloading(ctx: &Ctx, app_id: AppId) -> bool {
    ctx.state
        .downloads
        .get(&app_id)
        .is_some_and(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed))
        || ctx.state.download_queue.contains(&app_id)
}

fn same_lib(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().to_lowercase().replace('/', "\\");
    norm(a).trim_end_matches('\\') == norm(b).trim_end_matches('\\')
}

fn safe_backup_folder_name(name: &str, app_id: AppId) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || (c as u32) < 0x20 { ' ' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        format!("app_{app_id}")
    } else {
        format!("{trimmed} ({app_id})")
    }
}
