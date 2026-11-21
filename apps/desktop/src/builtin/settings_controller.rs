//! Settings → Controller: detected controllers, live input tester,
//! calibration & deadzones, global Steam Input family toggles, and per-game
//! Steam Input bindings inspector/selector.

use super::settings::switch;
use client::{
    BindingChoice, BindingOrigin, Command, ConnectionKind, ControllerFamily, ControllerSettings,
    DetectedController, InputConfigReport, PadCalibration,
};
use egui::{Align, Align2, CornerRadius, Layout, Margin, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use extensions::{Action, Ctx};
use i18n::t;
use model::AppId;
use std::path::PathBuf;
use std::time::Duration;
use ui::{Theme, icons, widgets as w};

#[derive(Default)]
pub struct ControllerPage {
    selected_controller: Option<String>,
    selected_app: Option<AppId>,
    inspected_app: Option<AppId>,
    inspected_choice: Option<BindingChoice>,
    inspector_loading: bool,
    report: Option<InputConfigReport>,
}

impl ControllerPage {
    /// Open with an optional navigation argument (`"controller"` or `"controller:<app_id>"`).
    pub fn open_arg(&mut self, arg: &str) {
        if let Some(id) = arg.strip_prefix("controller:").and_then(|s| s.parse::<AppId>().ok()) {
            self.selected_app = Some(id);
            self.inspected_app = None;
        }
    }

    /// Receive async inspection results from the Steam engine.
    pub fn on_event(&mut self, event: &client::Event) {
        if let client::Event::InputConfigInspected(report) = event
            && (self.selected_app == Some(report.app_id) || self.inspected_app == Some(report.app_id))
        {
            self.report = Some(report.clone());
            self.inspector_loading = false;
        }
    }

    pub fn show(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        w::page_title(
            ui,
            t,
            t!("Controller"),
            Some(t!("Detected controllers, live input testing, deadzones, Steam Input toggles and per-game bindings.")),
        );

        let mut cfg = client::controller_settings();
        let before = cfg.clone();

        let snapshot = client::poll_controllers();
        if !snapshot.controllers.is_empty() {
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        } else {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }

        self.detected_controllers_ui(ui, t, &snapshot, &cfg);
        ui.add_space(12.0);
        self.test_inputs_ui(ui, t, &snapshot, &cfg.calibration);
        ui.add_space(12.0);
        steam_input_families_ui(ui, t, &mut cfg);
        ui.add_space(12.0);
        calibration_ui(ui, t, &mut cfg.calibration);

        if cfg != before {
            client::set_controller_settings(cfg);
        }

        ui.add_space(12.0);
        self.bindings_inspector_ui(ui, t, ctx);
    }

    fn detected_controllers_ui(
        &mut self,
        ui: &mut Ui,
        t: &Theme,
        snapshot: &client::DetectionSnapshot,
        cfg: &ControllerSettings,
    ) {
        w::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(w::text_semibold(t!("Detected controllers"), 16.0, t.text));
                if !snapshot.backend.is_empty() {
                    w::pill(ui, snapshot.backend, t.text_dim, t.raised);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::button(ui, t, Some(icons::REFRESH), t!("Refresh"), w::Kind::Secondary, w::Size::Sm).clicked() {
                        client::refresh_controllers();
                    }
                });
            });
            ui.add_space(6.0);

            if snapshot.controllers.is_empty() {
                w::panel(t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(w::text(icons::CONTROLLER, 26.0, t.text_faint));
                        ui.vertical(|ui| {
                            ui.label(w::text_medium(t!("No controllers detected"), 14.0, t.text));
                            ui.add(
                                egui::Label::new(w::text(
                                    t!("Connect an Xbox, PlayStation, Switch Pro or generic gamepad via USB or Bluetooth."),
                                    12.5,
                                    t.text_dim,
                                ))
                                .wrap(),
                            );
                        });
                    });
                });
                if let Some(note) = &snapshot.note {
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(w::text(i18n::tr_dyn(note), 12.0, t.away)).wrap());
                }
                self.selected_controller = None;
                return;
            }

            if self
                .selected_controller
                .as_ref()
                .is_none_or(|id| !snapshot.controllers.iter().any(|c| &c.id == id))
            {
                self.selected_controller = snapshot.controllers.first().map(|c| c.id.clone());
            }

            for c in &snapshot.controllers {
                let selected = self.selected_controller.as_deref() == Some(c.id.as_str());
                let fill = if selected { w::active_fill(t) } else { t.raised };
                let stroke = if selected {
                    Stroke::new(1.5, t.accent)
                } else {
                    Stroke::new(1.0, t.border)
                };
                let frame = egui::Frame::new()
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(t.corner_small())
                    .inner_margin(Margin::symmetric(12, 10));
                let resp = frame.show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(w::text(icons::CONTROLLER, 22.0, if selected { t.accent } else { t.text_dim }));
                        ui.vertical(|ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(w::text_semibold(&c.name, 14.0, t.text));
                                let fam_label = family_label(c.family);
                                w::pill(ui, fam_label, t.accent, t.accent.gamma_multiply(0.14));
                                if cfg.families.is_enabled(c.family) {
                                    w::pill(ui, t!("Steam Input"), t.play, t.play.gamma_multiply(0.14));
                                } else {
                                    w::pill(ui, t!("Off globally"), t.text_faint, t.surface);
                                }
                            });
                            let status = controller_status_line(c);
                            ui.label(w::text(status, 12.0, t.text_dim));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if c.has_rumble
                                && cfg.calibration.rumble_enabled
                                && w::button(ui, t, None, t!("Vibrate"), w::Kind::Secondary, w::Size::Sm).clicked()
                            {
                                client::trigger_test_rumble(&c.id);
                            }
                        });
                    });
                });
                if resp.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.selected_controller = Some(c.id.clone());
                }
                ui.add_space(6.0);
            }

            if let Some(note) = &snapshot.note {
                ui.add(egui::Label::new(w::text(i18n::tr_dyn(note), 12.0, t.away)).wrap());
            }
        });
    }

    fn test_inputs_ui(
        &self,
        ui: &mut Ui,
        t: &Theme,
        snapshot: &client::DetectionSnapshot,
        cal: &PadCalibration,
    ) {
        let selected = self
            .selected_controller
            .as_deref()
            .and_then(|id| snapshot.controllers.iter().find(|c| c.id == id))
            .or_else(|| snapshot.controllers.first());

        w::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(w::text_semibold(t!("Test Device Inputs"), 16.0, t.text));
                if let Some(c) = selected {
                    ui.label(w::text(format!("·  {}", c.name), 13.0, t.text_dim));
                }
            });
            ui.add_space(6.0);

            let Some(c) = selected else {
                ui.label(w::text(t!("Select a connected controller above to inspect live sticks, triggers, buttons and motion sensors."), 13.0, t.text_dim));
                return;
            };

            let live = &c.live;
            let b = |mask: u16| live.buttons & mask != 0;
            let nintendo = cal.nintendo_layout;

            ui.columns(3, |cols| {
                // Left stick + LB + LT
                cols[0].vertical_centered(|ui| {
                    trigger_bar(ui, t, "LT", live.raw_lt, live.lt, cal.trigger_threshold);
                    ui.add_space(6.0);
                    bumper_pill(ui, t, "LB", b(0x0100));
                    ui.add_space(10.0);
                    stick_pad(
                        ui,
                        t,
                        t!("Left stick"),
                        live.raw_lx,
                        live.raw_ly,
                        live.lx,
                        live.ly,
                        cal.left_deadzone,
                        b(0x0040),
                    );
                });

                // Center: D-pad, center buttons, face buttons diamond, extra buttons
                cols[1].vertical_centered(|ui| {
                    ui.label(w::caption_text(t, t!("BUTTONS & D-PAD")));
                    ui.add_space(6.0);
                    dpad_cross(ui, t, b(0x0001), b(0x0002), b(0x0004), b(0x0008));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let total_w = 172.0;
                        let pad = ((ui.available_width() - total_w) * 0.5).max(0.0);
                        ui.add_space(pad);
                        button_pill(ui, t, "View", b(0x0020));
                        button_pill(ui, t, "Guide", b(0x0400));
                        button_pill(ui, t, "Menu", b(0x0010));
                    });
                    ui.add_space(10.0);
                    let (south, east, west, north) = if nintendo {
                        ("B", "A", "Y", "X")
                    } else {
                        ("A", "B", "X", "Y")
                    };
                    face_diamond(ui, t, (south, b(0x1000)), (east, b(0x2000)), (west, b(0x4000)), (north, b(0x8000)));
                    if live.misc || live.touchpad || live.paddles.iter().any(|p| *p) {
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
                            if live.misc {
                                button_pill(ui, t, "Misc", true);
                            }
                            if live.touchpad {
                                button_pill(ui, t, "Touchpad", true);
                            }
                            for (i, on) in live.paddles.iter().enumerate() {
                                if *on {
                                    button_pill(ui, t, &format!("P{}", i + 1), true);
                                }
                            }
                        });
                    }
                });

                // Right stick + RB + RT
                cols[2].vertical_centered(|ui| {
                    trigger_bar(ui, t, "RT", live.raw_rt, live.rt, cal.trigger_threshold);
                    ui.add_space(6.0);
                    bumper_pill(ui, t, "RB", b(0x0200));
                    ui.add_space(10.0);
                    stick_pad(
                        ui,
                        t,
                        t!("Right stick"),
                        live.raw_rx,
                        live.raw_ry,
                        live.rx,
                        live.ry,
                        cal.right_deadzone,
                        b(0x0080),
                    );
                });
            });

            ui.add_space(10.0);
            w::panel(t).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if !c.has_gyro {
                    ui.label(w::text(t!("Motion sensors (gyro / accelerometer): not available on this controller."), 12.5, t.text_dim));
                } else if !cal.gyro_enabled {
                    ui.label(w::text(t!("Motion sensors are disabled in Calibration & Deadzones below."), 12.5, t.away));
                } else {
                    ui.label(w::text_medium(t!("Motion sensors"), 13.0, t.text));
                    ui.add_space(4.0);
                    ui.columns(2, |cols| {
                        motion_axis_group(
                            &mut cols[0],
                            t,
                            t!("Gyroscope (rad/s)"),
                            live.gyro,
                            8.0,
                        );
                        motion_axis_group(
                            &mut cols[1],
                            t,
                            t!("Accelerometer (m/s²)"),
                            live.accel,
                            20.0,
                        );
                    });
                }
            });
        });
    }

    fn bindings_inspector_ui(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let games = available_games(ctx);
        if self.selected_app.is_none() {
            self.selected_app = games.first().map(|(id, _, _)| *id);
        }

        w::card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(w::text_semibold(t!("Per-game bindings"), 16.0, t.text));
            ui.add(
                egui::Label::new(w::text(
                    t!("Inspect Steam Input action sets or choose a controller configuration file for a game."),
                    12.5,
                    t.text_dim,
                ))
                .wrap(),
            );
            ui.add_space(8.0);

            if games.is_empty() {
                ui.label(w::text(t!("No games found in your library yet."), 13.0, t.text_dim));
                return;
            }

            let selected_id = self.selected_app.unwrap_or(games[0].0);
            let selected_entry = games.iter().find(|(id, _, _)| *id == selected_id).cloned().unwrap_or_else(|| games[0].clone());
            let (app_id, app_name, install_dir) = selected_entry;
            self.selected_app = Some(app_id);

            let current_choice = client::app_binding(app_id);
            let need_refresh = self.inspected_app != Some(app_id)
                || self.inspected_choice.as_ref() != Some(&current_choice);

            if need_refresh {
                self.inspect_now(ctx, app_id, &install_dir, &current_choice);
            }

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                egui::ComboBox::from_id_salt("ctrl-bindings-game")
                    .selected_text(&app_name)
                    .width(280.0)
                    .show_ui(ui, |ui| {
                        for (id, name, _) in &games {
                            ui.selectable_value(&mut self.selected_app, Some(*id), name);
                        }
                    });
                if w::button(ui, t, Some(icons::REFRESH), t!("Refresh"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    self.inspect_now(ctx, app_id, &install_dir, &current_choice);
                }
                if w::button(ui, t, Some(icons::SLIDERS), t!("Game Properties…"), w::Kind::Ghost, w::Size::Sm).clicked() {
                    ctx.act(Action::Navigate { page: "library".into(), arg: Some(format!("props:{app_id}")) });
                }
            });

            ui.add_space(8.0);
            let Some(report) = &self.report else { return };

            // Config selector row
            let mut picked = current_choice.clone();
            let chosen_label = match &picked {
                BindingChoice::Auto => t!("Automatic (default)").to_string(),
                BindingChoice::LocalFile(p) => report
                    .available
                    .iter()
                    .find(|o| o.path.as_ref() == Some(p))
                    .map(|o| option_label(o))
                    .unwrap_or_else(|| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
                BindingChoice::Workshop(id) => report
                    .available
                    .iter()
                    .find(|o| o.workshop_id == Some(*id))
                    .map(|o| option_label(o))
                    .unwrap_or_else(|| t!("Workshop #{id}", id = id)),
            };

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                ui.label(w::text_medium(t!("Configuration:"), 13.0, t.text));
                egui::ComboBox::from_id_salt(("ctrl-bindings-cfg", app_id))
                    .selected_text(chosen_label)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut picked, BindingChoice::Auto, t!("Automatic (default)"));
                        for opt in &report.available {
                            ui.selectable_value(&mut picked, opt.choice.clone(), option_label(opt));
                        }
                    });
                if w::button(ui, t, Some(icons::FOLDER), t!("Browse .vdf…"), w::Kind::Secondary, w::Size::Sm).clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("Steam Input VDF", &["vdf"]).pick_file() {
                        let valid = std::fs::read_to_string(&path)
                            .ok()
                            .is_some_and(|txt| !client::parse_vdf_summary(&txt).1.is_empty() || txt.contains("controller_mappings"));
                        if valid {
                            picked = BindingChoice::LocalFile(path);
                        } else {
                            ctx.toast(t!("That file does not look like a Steam Input controller configuration."));
                        }
                    }
                }
                if let Some(path) = report.active_path.as_ref() {
                    if w::button(ui, t, Some(icons::EXTERNAL), t!("Open"), w::Kind::Secondary, w::Size::Sm).clicked() {
                        let _ = open::that(path);
                    }
                } else if let Some(dir) = report.config_dir.as_ref() {
                    if w::button(ui, t, Some(icons::FOLDER), t!("Open folder"), w::Kind::Secondary, w::Size::Sm).clicked() {
                        let _ = std::fs::create_dir_all(dir);
                        local::open_folder(dir);
                    }
                }
                if current_choice != BindingChoice::Auto
                    && w::button(ui, t, Some(icons::RESTORE), t!("Reset"), w::Kind::Ghost, w::Size::Sm).clicked()
                {
                    picked = BindingChoice::Auto;
                }
            });

            let binding_changed = picked != current_choice;

            ui.add_space(8.0);
            w::panel(t).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    if self.inspector_loading {
                        ui.spinner();
                    }
                    if report.active_source.is_empty() {
                        ui.label(w::text(t!("No Steam Input bindings found for this game (the game uses its own controller support)."), 13.0, t.text_dim));
                    } else {
                        ui.label(w::text_medium(t!("Active source: {source}", source = &report.active_source), 13.0, t.text));
                    }
                    if !report.active_controller_type.is_empty() {
                        w::pill(ui, &report.active_controller_type, t.accent, t.accent.gamma_multiply(0.14));
                    }
                });

                if !report.action_sets.is_empty() {
                    ui.add_space(8.0);
                    for set in &report.action_sets {
                        egui::Frame::new()
                            .fill(t.surface)
                            .corner_radius(t.corner_small())
                            .inner_margin(Margin::symmetric(10, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let heading = if set.title.is_empty() || set.title.starts_with('#') {
                                        &set.name
                                    } else {
                                        &set.title
                                    };
                                    ui.label(w::text_semibold(heading, 13.5, t.text));
                                    let badge = if set.is_layer { t!("Action layer") } else { t!("Action set") };
                                    w::pill(ui, badge, t.text_dim, t.raised);
                                });
                                if set.bindings.is_empty() {
                                    ui.label(w::text(t!("No mapped game actions in this set."), 12.0, t.text_faint));
                                } else {
                                    ui.add_space(4.0);
                                    for (source, input, acts) in set.bindings.iter().take(16) {
                                        ui.horizontal(|ui| {
                                            ui.label(w::text(format!("{source} · {input}"), 12.0, t.text_dim).monospace());
                                            ui.label(w::text("→", 12.0, t.text_faint));
                                            ui.label(w::text(acts.join(", "), 12.0, t.text));
                                        });
                                    }
                                    if set.bindings.len() > 16 {
                                        ui.label(w::text(t!("and {n} more bindings", n = set.bindings.len() - 16), 11.5, t.text_faint));
                                    }
                                }
                            });
                        ui.add_space(6.0);
                    }
                }
            });

            if binding_changed {
                client::set_app_binding(app_id, picked.clone());
                self.inspect_now(ctx, app_id, &install_dir, &picked);
            }
        });
    }

    fn inspect_now(&mut self, ctx: &Ctx, app_id: AppId, install_dir: &PathBuf, choice: &BindingChoice) {
        let account = (ctx.state.steam_id & 0xFFFF_FFFF) as u32;
        self.report = Some(client::inspect_input_config_local(&ctx.local.steam_root, account, app_id, install_dir));
        self.inspected_app = Some(app_id);
        self.inspected_choice = Some(choice.clone());
        if ctx.state.phase == client::state::Phase::LoggedIn && ctx.state.link == client::Link::Online {
            self.inspector_loading = true;
            ctx.send(Command::InspectInputConfig { app_id, install_dir: install_dir.clone() });
        } else {
            self.inspector_loading = false;
        }
    }
}

fn steam_input_families_ui(ui: &mut Ui, t: &Theme, cfg: &mut ControllerSettings) {
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(w::text_semibold(t!("Steam Input"), 16.0, t.text));
        ui.add(
            egui::Label::new(w::text(
                t!("Choose which controller families Aurora handles through Steam Input and translates for games that expect Xbox controllers."),
                12.5,
                t.text_dim,
            ))
            .wrap(),
        );
        ui.add_space(8.0);
        switch(
            ui,
            t,
            t!("Enable Steam Input for Xbox controllers"),
            t!("Use Aurora's Steam Input action mapping for Xbox controllers when a game uses Steam Input."),
            &mut cfg.families.xbox,
        );
        switch(
            ui,
            t,
            t!("Enable Steam Input for PlayStation controllers"),
            t!("Read DualShock and DualSense controllers (including gyro) and expose them to games."),
            &mut cfg.families.playstation,
        );
        switch(
            ui,
            t,
            t!("Enable Steam Input for Switch Pro controllers"),
            t!("Read Nintendo Switch Pro and Joy-Con controllers and expose them to games."),
            &mut cfg.families.switch_pro,
        );
        switch(
            ui,
            t,
            t!("Enable Steam Input for generic controllers"),
            t!("Read DirectInput and third-party gamepads as Xbox-style controllers."),
            &mut cfg.families.generic,
        );
    });
}

fn calibration_ui(ui: &mut Ui, t: &Theme, cal: &mut PadCalibration) {
    w::card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(w::text_semibold(t!("Calibration & Deadzones"), 16.0, t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if w::button(ui, t, Some(icons::RESTORE), t!("Reset to defaults"), w::Kind::Ghost, w::Size::Sm).clicked() {
                    *cal = PadCalibration::default();
                }
            });
        });
        ui.add_space(8.0);

        egui::Grid::new("controller-calibration-grid")
            .num_columns(2)
            .spacing(vec2(24.0, 10.0))
            .show(ui, |ui| {
                ui.label(w::text_medium(t!("Left stick deadzone"), 13.5, t.text));
                ui.add(
                    egui::Slider::new(&mut cal.left_deadzone, 0.0..=0.50)
                        .step_by(0.01)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                ui.end_row();

                ui.label(w::text_medium(t!("Right stick deadzone"), 13.5, t.text));
                ui.add(
                    egui::Slider::new(&mut cal.right_deadzone, 0.0..=0.50)
                        .step_by(0.01)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                ui.end_row();

                ui.label(w::text_medium(t!("Trigger threshold"), 13.5, t.text));
                ui.add(
                    egui::Slider::new(&mut cal.trigger_threshold, 0.0..=0.50)
                        .step_by(0.01)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                ui.end_row();

                ui.label(w::text_medium(t!("Gyro sensitivity"), 13.5, t.text));
                ui.add_enabled(
                    cal.gyro_enabled,
                    egui::Slider::new(&mut cal.gyro_sensitivity, 0.1..=4.0)
                        .step_by(0.05)
                        .custom_formatter(|v, _| format!("{v:.2}×")),
                );
                ui.end_row();
            });

        ui.add_space(10.0);
        switch(
            ui,
            t,
            t!("Enable gyroscope & motion sensors"),
            t!("Forward controller gyro and accelerometer data to games that request motion."),
            &mut cal.gyro_enabled,
        );
        switch(
            ui,
            t,
            t!("Controller vibration (rumble)"),
            t!("Allow games and input tests to trigger controller vibration motors."),
            &mut cal.rumble_enabled,
        );
        switch(
            ui,
            t,
            t!("Use Nintendo button layout"),
            t!("Swap A/B and X/Y face buttons to match Nintendo Switch controller labels."),
            &mut cal.nintendo_layout,
        );
    });
}

fn family_label(f: ControllerFamily) -> &'static str {
    match f {
        ControllerFamily::Xbox => t!("Xbox"),
        ControllerFamily::PlayStation => t!("PlayStation"),
        ControllerFamily::SwitchPro => t!("Switch Pro"),
        ControllerFamily::Generic => t!("Generic"),
    }
}

fn controller_status_line(c: &DetectedController) -> String {
    let slot = match c.slot {
        Some(s) => t!("Slot {n}", n = s + 1),
        None => t!("Standby").to_string(),
    };
    let conn = match c.connection {
        ConnectionKind::Wired => t!("Wired"),
        ConnectionKind::Wireless => t!("Wireless"),
        ConnectionKind::Unknown => t!("Connected"),
    };
    let mut parts = vec![slot, conn.to_string()];
    if let Some(pct) = c.battery_percent {
        parts.push(t!("Battery {pct}%", pct = pct));
    }
    if c.vendor_id != 0 || c.product_id != 0 {
        parts.push(format!("{:04X}:{:04X}", c.vendor_id, c.product_id));
    }
    parts.join("  ·  ")
}

fn option_label(opt: &client::BindingOption) -> String {
    match opt.origin {
        BindingOrigin::UserFile => t!("User: {name}", name = opt.label.strip_prefix("User: ").unwrap_or(&opt.label)),
        BindingOrigin::UserWorkshop(id) => t!("Your Workshop #{id}", id = id),
        BindingOrigin::GameFile => t!("Game: {name}", name = opt.label.strip_prefix("Game: ").unwrap_or(&opt.label)),
        BindingOrigin::Official(id) if opt.controller_type.is_empty() => t!("Official Workshop #{id}", id = id),
        BindingOrigin::Official(id) => format!("{} ({})", t!("Official Workshop #{id}", id = id), opt.controller_type),
        BindingOrigin::CustomFile => t!("Custom: {name}", name = opt.label.strip_prefix("Custom: ").unwrap_or(&opt.label)),
    }
}

fn available_games(ctx: &Ctx) -> Vec<(AppId, String, PathBuf)> {
    let mut list: Vec<(AppId, String, PathBuf, bool)> = Vec::new();
    for g in &ctx.local.games {
        list.push((g.app_id, g.name.clone(), g.install_dir.clone(), true));
    }
    if let Some(apps) = &ctx.state.apps {
        for a in apps.values().filter(|a| !a.is_dlc() && !a.name.is_empty()) {
            if !list.iter().any(|(id, _, _, _)| *id == a.app_id) {
                list.push((a.app_id, a.name.clone(), PathBuf::new(), false));
            }
        }
    }
    list.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    list.into_iter().map(|(id, name, dir, _)| (id, name, dir)).collect()
}

// ------------------------------------------------------------ Live visualizer widgets

fn stick_pad(ui: &mut Ui, t: &Theme, label: &str, raw_x: f32, raw_y: f32, cal_x: f32, cal_y: f32, deadzone: f32, clicked: bool) {
    ui.label(w::text_medium(label, 12.5, t.text_dim));
    let radius = 44.0;
    let (rect, _) = ui.allocate_exact_size(vec2(radius * 2.0 + 10.0, radius * 2.0 + 10.0), Sense::hover());
    let c = rect.center();
    let p = ui.painter();

    p.circle_filled(c, radius, t.raised);
    p.circle_stroke(c, radius, Stroke::new(1.5, if clicked { t.accent } else { t.border }));
    // Axes crosshair
    p.line_segment([pos2(c.x - radius, c.y), pos2(c.x + radius, c.y)], Stroke::new(1.0, t.border));
    p.line_segment([pos2(c.x, c.y - radius), pos2(c.x, c.y + radius)], Stroke::new(1.0, t.border));
    // Deadzone ring
    let dz_r = radius * deadzone.clamp(0.0, 0.95);
    if dz_r > 1.0 {
        p.circle_filled(c, dz_r, t.accent.gamma_multiply(0.10));
        p.circle_stroke(c, dz_r, Stroke::new(1.0, t.accent.gamma_multiply(0.55)));
    }
    // Raw stick dot (subtle)
    let raw_pos = pos2(c.x + raw_x.clamp(-1.0, 1.0) * radius, c.y - raw_y.clamp(-1.0, 1.0) * radius);
    p.circle_filled(raw_pos, 3.5, t.text_faint);
    // Calibrated stick dot
    let cal_pos = pos2(c.x + cal_x.clamp(-1.0, 1.0) * radius, c.y - cal_y.clamp(-1.0, 1.0) * radius);
    p.circle_filled(cal_pos, 6.0, if clicked { t.play } else { t.accent });

    ui.label(w::text(format!("{cal_x:+.2}, {cal_y:+.2}"), 11.5, t.text_dim).monospace());
    button_pill(ui, t, if label == t!("Left stick") { "LS" } else { "RS" }, clicked);
}

fn trigger_bar(ui: &mut Ui, t: &Theme, label: &str, raw: f32, cal: f32, threshold: f32) {
    ui.horizontal(|ui| {
        ui.label(w::text_medium(label, 12.0, t.text_dim));
        let (rect, _) = ui.allocate_exact_size(vec2(110.0, 14.0), Sense::hover());
        let p = ui.painter();
        let r = CornerRadius::same(4);
        p.rect_filled(rect, r, t.raised);
        if raw > 0.0 {
            let w_raw = rect.width() * raw.clamp(0.0, 1.0);
            p.rect_filled(Rect::from_min_size(rect.min, vec2(w_raw, rect.height())), r, t.text_faint.gamma_multiply(0.35));
        }
        if cal > 0.0 {
            let w_cal = rect.width() * cal.clamp(0.0, 1.0);
            p.rect_filled(Rect::from_min_size(rect.min, vec2(w_cal, rect.height())), r, t.accent);
        }
        let th_x = rect.left() + rect.width() * threshold.clamp(0.0, 0.95);
        p.line_segment([pos2(th_x, rect.top()), pos2(th_x, rect.bottom())], Stroke::new(1.5, t.away));
        p.rect_stroke(rect, r, Stroke::new(1.0, t.border), StrokeKind::Inside);
        ui.label(w::text(format!("{:.0}%", cal * 100.0), 11.5, t.text_dim).monospace());
    });
}

fn bumper_pill(ui: &mut Ui, t: &Theme, label: &str, active: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(88.0, 22.0), Sense::hover());
    paint_key_box(ui.painter(), t, rect, label, active, CornerRadius::same(6));
}

fn button_pill(ui: &mut Ui, t: &Theme, label: &str, active: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(52.0, 22.0), Sense::hover());
    paint_key_box(ui.painter(), t, rect, label, active, CornerRadius::same(5));
}

fn dpad_cross(ui: &mut Ui, t: &Theme, up: bool, down: bool, left: bool, right: bool) {
    let sz = 22.0;
    let (rect, _) = ui.allocate_exact_size(vec2(sz * 3.0, sz * 3.0), Sense::hover());
    let p = ui.painter();
    let cell = |gx: f32, gy: f32| Rect::from_min_size(pos2(rect.left() + gx * sz, rect.top() + gy * sz), vec2(sz - 2.0, sz - 2.0));
    paint_key_box(p, t, cell(1.0, 0.0), "▲", up, CornerRadius::same(4));
    paint_key_box(p, t, cell(1.0, 2.0), "▼", down, CornerRadius::same(4));
    paint_key_box(p, t, cell(0.0, 1.0), "◀", left, CornerRadius::same(4));
    paint_key_box(p, t, cell(2.0, 1.0), "▶", right, CornerRadius::same(4));
    p.rect_filled(cell(1.0, 1.0), CornerRadius::same(3), t.raised);
}

fn face_diamond(
    ui: &mut Ui,
    t: &Theme,
    south: (&str, bool),
    east: (&str, bool),
    west: (&str, bool),
    north: (&str, bool),
) {
    let sz = 26.0;
    let (rect, _) = ui.allocate_exact_size(vec2(sz * 3.0, sz * 3.0), Sense::hover());
    let p = ui.painter();
    let dot = |gx: f32, gy: f32, (lbl, on): (&str, bool)| {
        let c = pos2(rect.left() + (gx + 0.5) * sz, rect.top() + (gy + 0.5) * sz);
        let r = sz * 0.44;
        p.circle_filled(c, r, if on { t.accent } else { t.raised });
        p.circle_stroke(c, r, Stroke::new(1.2, if on { t.accent } else { t.border }));
        p.text(c, Align2::CENTER_CENTER, lbl, w::font_bold(11.5), if on { t.on_accent } else { t.text });
    };
    dot(1.0, 0.0, north);
    dot(2.0, 1.0, east);
    dot(0.0, 1.0, west);
    dot(1.0, 2.0, south);
}

fn paint_key_box(p: &egui::Painter, t: &Theme, rect: Rect, label: &str, active: bool, radius: CornerRadius) {
    p.rect_filled(rect, radius, if active { t.accent } else { t.raised });
    p.rect_stroke(rect, radius, Stroke::new(1.0, if active { t.accent } else { t.border }), StrokeKind::Inside);
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        w::font_semibold(11.0),
        if active { t.on_accent } else { t.text_dim },
    );
}

fn motion_axis_group(ui: &mut Ui, t: &Theme, title: &str, vals: [f32; 3], scale: f32) {
    ui.label(w::text_medium(title, 12.0, t.text_dim));
    for (name, val) in [("X", vals[0]), ("Y", vals[1]), ("Z", vals[2])] {
        ui.horizontal(|ui| {
            ui.label(w::text(name, 11.5, t.text_faint).monospace());
            let (rect, _) = ui.allocate_exact_size(vec2(140.0, 10.0), Sense::hover());
            let p = ui.painter();
            let r = CornerRadius::same(3);
            p.rect_filled(rect, r, t.surface);
            let norm = (val / scale).clamp(-1.0, 1.0);
            let mid = rect.center().x;
            let bar = if norm >= 0.0 {
                Rect::from_min_max(pos2(mid, rect.top()), pos2(mid + rect.width() * 0.5 * norm, rect.bottom()))
            } else {
                Rect::from_min_max(pos2(mid + rect.width() * 0.5 * norm, rect.top()), pos2(mid, rect.bottom()))
            };
            p.rect_filled(bar, r, t.accent);
            p.line_segment([pos2(mid, rect.top()), pos2(mid, rect.bottom())], Stroke::new(1.0, t.border));
            ui.label(w::text(format!("{val:+.2}"), 11.5, t.text_dim).monospace());
        });
    }
}
