//! Store: interactive Discovery Queue walkthrough, ported and adapted from
//! upstream work by `madonchik123` (PR #62 `731cef2`).
//!
//! Steps through recommended games one at a time with Previous, Follow,
//! Not interested, Wishlist, Next in queue, and Start another queue.

use super::{Store, View};
use client::Command;
use egui::{Align, Layout, Margin, Ui};
use extensions::Ctx;
use i18n::t;
use model::AppId;
use ui::{Theme, icons, widgets as w};

/// How long to wait for Steam to send a queue before timing out.
const QUEUE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Sample discovery queue apps when offline `--demo` is rebuilt locally.
const DEMO_QUEUE: [AppId; 6] = [1145360, 413150, 367520, 646570, 268910, 477160];

impl Store {
    pub(super) fn ask_queue(&mut self, ctx: &Ctx, rebuild: bool) {
        if ctx.state.account == "demo" {
            self.discovery_wait = false;
            self.discovery_late = false;
            self.discovery_asked = None;
            let next = match &mut self.discovery {
                Some(Ok(q)) if !q.is_empty() => {
                    if rebuild && q.len() > 1 {
                        q.rotate_left(1);
                    }
                    q.clone()
                }
                _ => DEMO_QUEUE.to_vec(),
            };
            self.discovery = Some(Ok(next));
            return;
        }
        self.discovery_wait = true;
        self.discovery_late = false;
        self.discovery_asked = Some(std::time::Instant::now());
        ctx.send(Command::LoadDiscoveryQueue { country: ctx.state.country.clone(), rebuild });
    }

    fn queue_state(&mut self, ctx: &Ctx) -> Option<Result<Vec<AppId>, String>> {
        if self.discovery.is_none() && !self.discovery_wait && !self.discovery_late {
            self.ask_queue(ctx, false);
        }
        if self.discovery_wait {
            let asked = self.discovery_asked.get_or_insert_with(std::time::Instant::now);
            if asked.elapsed() < QUEUE_WAIT {
                self.egui.request_repaint_after(std::time::Duration::from_secs(1));
                return None;
            }
            self.discovery_wait = false;
            self.discovery_asked = None;
            self.discovery_late = true;
        }
        if self.discovery_late {
            return Some(Err(t!("Steam didn't send a queue. Try again in a moment.").to_string()));
        }
        self.discovery.clone()
    }

    fn is_followed(&self, ctx: &Ctx, id: AppId) -> bool {
        if ctx.state.account == "demo" {
            if let Some(&v) = self.demo_followed.get(&id) {
                return v;
            }
        }
        ctx.state.followed_apps.contains(&id)
    }

    fn is_ignored(&self, ctx: &Ctx, id: AppId) -> bool {
        if ctx.state.account == "demo" {
            if let Some(&v) = self.demo_ignored.get(&id) {
                return v;
            }
        }
        ctx.state.ignored_apps.contains(&id)
    }

    /// Walkthrough bar at `pos` above the game's store page, or the end card.
    pub(super) fn discovery_page(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, pos: usize) {
        let queue: Vec<AppId> = match self.queue_state(ctx) {
            Some(Ok(q)) => q,
            Some(Err(e)) => {
                egui::Frame::new().inner_margin(Margin::same(32)).show(ui, |ui| {
                    w::empty(ui, t, icons::WARNING, t!("Couldn't load your discovery queue"), &e);
                    ui.add_space(10.0);
                    ui.vertical_centered(|ui| {
                        if w::button(ui, t, Some(icons::REFRESH), t!("Try again"), w::Kind::Primary, w::Size::Md).clicked() {
                            self.ask_queue(ctx, false);
                        }
                    });
                });
                return;
            }
            None => {
                ui.add_space(40.0);
                w::loading(ui, t, t!("Loading your discovery queue…"));
                return;
            }
        };
        let Some(&id) = queue.get(pos) else {
            self.discovery_done(ui, t, ctx, queue.len());
            return;
        };

        let demo = ctx.state.account == "demo";
        w::centered(ui, 1200.0, 28.0, |ui| {
            egui::Frame::new()
                .fill(t.bg.gamma_multiply(0.88))
                .stroke(egui::Stroke::new(1.0, t.border.gamma_multiply(0.7)))
                .corner_radius(t.corner_small())
                .inner_margin(Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                        ui.label(w::text(icons::STACK, 18.0, t.accent));
                        ui.vertical(|ui| {
                            ui.label(w::text_semibold(t!("Your discovery queue"), 15.0, t.text));
                            ui.label(w::text(t!("{n} of {total}", n = pos + 1, total = queue.len()), 12.5, t.text_dim));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                            let last = pos + 1 == queue.len();
                            let label = if last { t!("Finish queue") } else { t!("Next in queue") };
                            if w::button(ui, t, Some(icons::ARROW_RIGHT), label, w::Kind::Primary, w::Size::Md).clicked() {
                                if !demo {
                                    ctx.send(Command::SkipDiscoveryQueueItem { app_id: id });
                                }
                                self.open(View::Discovery(pos + 1));
                            }
                            let wished = self.is_wishlisted(ctx, id);
                            let owned = self.owned.contains(&id);
                            if !owned {
                                let (icon, label) = if wished { (icons::CHECK, t!("On wishlist")) } else { (icons::HEART, t!("Add to wishlist")) };
                                if w::button(ui, t, Some(icon), label, w::Kind::Secondary, w::Size::Md).clicked() {
                                    self.toggle_wishlist(ctx, id, wished);
                                }
                            }
                            let followed = self.is_followed(ctx, id);
                            let (icon, label) = if followed { (icons::CHECK, t!("Following")) } else { (icons::BELL, t!("Follow")) };
                            if w::button(ui, t, Some(icon), label, w::Kind::Secondary, w::Size::Md).clicked() {
                                if demo {
                                    self.demo_followed.insert(id, !followed);
                                } else {
                                    ctx.send(Command::FollowApp { app_id: id, follow: !followed });
                                }
                            }
                            let ignored = self.is_ignored(ctx, id);
                            let (icon, label) = if ignored { (icons::EYE, t!("Interested again")) } else { (icons::EYE_SLASH, t!("Not interested")) };
                            if w::button(ui, t, Some(icon), label, w::Kind::Ghost, w::Size::Md).clicked() {
                                if demo {
                                    self.demo_ignored.insert(id, !ignored);
                                } else {
                                    ctx.send(Command::IgnoreApp { app_id: id, ignore: !ignored });
                                    if !ignored {
                                        ctx.send(Command::SkipDiscoveryQueueItem { app_id: id });
                                    }
                                }
                                if !ignored {
                                    self.open(View::Discovery(pos + 1));
                                }
                            }
                            if pos > 0 && w::button(ui, t, Some(icons::ARROW_LEFT), t!("Previous"), w::Kind::Ghost, w::Size::Md).clicked() {
                                self.open(View::Discovery(pos - 1));
                            }
                        });
                    });
                });
        });

        if !self.allowed.contains(&id) && self.details_mature(id) {
            self.mature_gate(ui, t, id);
        } else {
            self.app_page(ui, t, ctx, id);
        }
    }

    /// End of the queue: start another queue, or return to Store Home.
    fn discovery_done(&mut self, ui: &mut Ui, t: &Theme, ctx: &Ctx, count: usize) {
        ui.add_space(56.0);
        w::centered(ui, 640.0, 28.0, |ui| {
            w::card(t).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.add_space(10.0);
                    ui.label(w::text(icons::CHECK, 34.0, t.play));
                    ui.add_space(6.0);
                    let title = if count == 0 { t!("Your discovery queue is empty") } else { t!("You've been through your queue") };
                    ui.label(w::text_bold(title, 20.0, t.text));
                    ui.add_space(4.0);
                    ui.label(w::text(t!("Steam picks a new set of games for you each time."), 13.5, t.text_dim));
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        ui.add_space((ui.available_width() - 360.0).max(0.0) / 2.0);
                        if w::button(ui, t, Some(icons::REFRESH), t!("Start another queue"), w::Kind::Primary, w::Size::Md).clicked() {
                            self.ask_queue(ctx, true);
                            self.open(View::Discovery(0));
                        }
                        if w::button(ui, t, Some(icons::ARROW_LEFT), t!("Back to the store"), w::Kind::Ghost, w::Size::Md).clicked() {
                            self.open(View::Home);
                        }
                    });
                    ui.add_space(10.0);
                });
            });
        });
    }
}
