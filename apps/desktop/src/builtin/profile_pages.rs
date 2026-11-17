//! A profile's badges, reviews and groups, each as its own list (like the
//! games list), from the profile's pages on steamcommunity.com.

use super::{Profile, Section, badge_icon, level_badge, panel};
use client::Command;
use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Ui, pos2, vec2};
use extensions::Ctx;
use i18n::t;
use model::SteamId;
use ui::{Theme, icons, widgets as w};

/// Reviews per page on Steam.
const REVIEWS_PER_PAGE: u32 = 10;

impl Profile {
    /// Badges, reviews or groups, with a way back to the profile.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn list_page(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, name: &str, mine: bool, section: Section) {
        ui.horizontal(|ui| {
            if w::button(ui, t, Some(icons::ARROW_LEFT), t!("Back to profile"), w::Kind::Ghost, w::Size::Sm).clicked() {
                ctx.navigate("profile", Some(id.to_string()));
            }
        });
        ui.add_space(8.0);
        panel(ui, t, |ui| match section {
            Section::Badges => self.badges_list(ui, t, ctx, id, name, mine),
            Section::Reviews => self.reviews_list(ui, t, ctx, id, name, mine),
            Section::Groups => self.groups_list(ui, t, ctx, id, name, mine),
            Section::Profile | Section::Games => {}
        });
    }

    fn badges_list(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, name: &str, mine: bool) {
        self.request_social(ctx, id);
        let title = if mine { t!("Your badges").to_string() } else { t!("{name}'s badges", name = name) };
        ui.label(w::text_bold(title, 24.0, t.text));
        let b = match ctx.state.badges.get(&id).cloned() {
            None => return skeleton_rows(ui, t, 70.0),
            Some(Err(e)) => return hidden(ui, t, &e, t!("These badges are hidden")),
            Some(Ok(b)) => b,
        };
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            level_badge(ui, t, b.level);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(w::text_bold(t!("Level {n}", n = b.level), 16.0, t.text));
                let sub = [b.xp.as_str(), b.next.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                ui.label(w::text(sub, 12.5, t.text_dim));
            });
        });
        if !b.next.is_empty() {
            ui.add_space(4.0);
            let width = ui.available_width().min(420.0);
            w::progress(ui, t, b.progress, width);
        }
        ui.add_space(12.0);
        ui.label(w::text(t!("{n} badges", n = b.items.len()), 13.5, t.text_dim));
        ui.add_space(6.0);
        if b.items.is_empty() {
            ui.label(w::text(t!("No badges yet."), 13.5, t.text_faint));
        }
        for badge in &b.items {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 72.0), Sense::click());
            let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
            ui.painter().rect_filled(r, t.corner_small(), w::mix(Color32::TRANSPARENT, t.surface_hover, hover));
            let mut icon_ui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(r.min + vec2(6.0, 6.0), vec2(60.0, 60.0))));
            badge_icon(&mut icon_ui, t, badge, 60.0);
            let x = r.left() + 80.0;
            let p = ui.painter();
            p.text(pos2(x, r.top() + 20.0), Align2::LEFT_CENTER, &badge.title, w::font_semibold(14.0), t.text);
            p.text(pos2(x, r.top() + 38.0), Align2::LEFT_CENTER, &badge.detail, w::font(12.5), t.accent);
            p.text(pos2(x, r.top() + 55.0), Align2::LEFT_CENTER, &badge.unlocked, w::font(11.5), t.text_faint);
            if !badge.url.is_empty() && resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(t!("Opens in your browser")).clicked() {
                let _ = open::that_detached(&badge.url);
            }
        }
    }

    fn reviews_list(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, name: &str, mine: bool) {
        let page = self.review_page.max(1);
        if self.lists_requested.insert((id, Section::Reviews, page)) {
            ctx.send(Command::LoadReviews { steam_id: id, page });
        }
        let title = if mine { t!("Your reviews").to_string() } else { t!("{name}'s reviews", name = name) };
        ui.label(w::text_bold(title, 24.0, t.text));
        let reviews = match ctx.state.reviews.get(&(id, page)).cloned() {
            None => return skeleton_rows(ui, t, 120.0),
            Some(Err(e)) => return hidden(ui, t, &e, t!("These reviews are hidden")),
            Some(Ok(r)) => r,
        };
        ui.label(w::text(t!("{n} reviews", n = reviews.total), 13.5, t.text_dim));
        ui.add_space(10.0);
        if reviews.items.is_empty() {
            w::empty(ui, t, icons::THUMBS_UP, t!("No reviews yet"), "");
            return;
        }
        for r in &reviews.items {
            review_card(ui, t, ctx, r);
            ui.add_space(8.0);
        }
        let pages = reviews.total.div_ceil(REVIEWS_PER_PAGE).max(1);
        if pages > 1 {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add_enabled_ui(page > 1, |ui| w::button(ui, t, Some(icons::CARET_LEFT), t!("Previous"), w::Kind::Secondary, w::Size::Sm)).inner.clicked() {
                    self.review_page = page - 1;
                }
                ui.label(w::text(t!("Page {n} of {total}", n = page, total = pages), 13.0, t.text_dim));
                if ui.add_enabled_ui(page < pages, |ui| w::button(ui, t, Some(icons::CARET_RIGHT), t!("Next"), w::Kind::Secondary, w::Size::Sm)).inner.clicked() {
                    self.review_page = page + 1;
                }
            });
        }
    }

    fn groups_list(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, id: SteamId, name: &str, mine: bool) {
        if self.lists_requested.insert((id, Section::Groups, 0)) {
            ctx.send(Command::LoadGroups { steam_id: id });
        }
        let title = if mine { t!("Your groups").to_string() } else { t!("{name}'s groups", name = name) };
        ui.label(w::text_bold(title, 24.0, t.text));
        let groups = match ctx.state.groups.get(&id).cloned() {
            None => return skeleton_rows(ui, t, 64.0),
            Some(Err(e)) => return hidden(ui, t, &e, t!("These groups are hidden")),
            Some(Ok(g)) => g,
        };
        ui.label(w::text(t!("{n} groups", n = groups.len()), 13.5, t.text_dim));
        ui.add_space(10.0);
        if groups.is_empty() {
            w::empty(ui, t, icons::USERS_THREE, t!("Not in any groups"), "");
            return;
        }
        for g in &groups {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::click());
            let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
            ui.painter().rect_filled(r, t.corner_small(), w::mix(Color32::TRANSPARENT, t.surface_hover, hover));
            let img = Rect::from_min_size(r.min + vec2(6.0, 6.0), vec2(52.0, 52.0));
            match &g.avatar {
                Some(u) => w::image_or_skeleton(ui, t, img, u, CornerRadius::same(4)),
                None => w::placeholder(ui, t, img, "", CornerRadius::same(4)),
            }
            let x = img.right() + 12.0;
            let sub = [g.members.as_str(), g.online.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            let p = ui.painter();
            p.text(pos2(x, r.top() + 22.0), Align2::LEFT_CENTER, &g.name, w::font_semibold(14.5), t.text);
            p.text(pos2(x, r.top() + 42.0), Align2::LEFT_CENTER, sub, w::font(12.5), t.text_dim);
            p.text(pos2(r.right() - 12.0, r.center().y), Align2::RIGHT_CENTER, icons::EXTERNAL, w::font(12.0), t.text_faint);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(t!("Opens in your browser")).clicked() {
                let _ = open::that_detached(&g.url);
            }
        }
    }
}

/// One review: the game's capsule (opens it), the verdict, playtime, text.
fn review_card(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, r: &model::UserReview) {
    egui::Frame::new().fill(t.surface.gamma_multiply(0.7)).corner_radius(t.corner_small()).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let (img, resp) = ui.allocate_exact_size(vec2(184.0, 69.0), Sense::click());
            match &r.capsule {
                Some(u) => w::image_or_skeleton(ui, t, img, u, t.corner_small()),
                None => w::image_or_skeleton(ui, t, img, &model::art::header(r.app_id), t.corner_small()),
            }
            let owned = ctx.state.apps.as_ref().is_some_and(|a| a.contains_key(&r.app_id));
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                if owned {
                    ctx.navigate("library", Some(r.app_id.to_string()));
                } else {
                    ctx.navigate("store", Some(format!("app:{}", r.app_id)));
                }
            }
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                let (icon, verdict, color) = if r.recommended { (icons::THUMBS_UP, t!("Recommended"), t.online) } else { (icons::THUMBS_DOWN, t!("Not Recommended"), t.danger) };
                ui.horizontal(|ui| {
                    ui.label(w::text(icon, 18.0, color));
                    ui.label(w::text_bold(verdict, 16.0, t.text));
                    if let Some(name) = ctx.state.app_name(r.app_id) {
                        ui.label(w::text(format!("·  {name}"), 13.5, t.text_dim));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !r.url.is_empty() && w::icon_button(ui, t, icons::EXTERNAL, t!("Open on Steam"), false, 0).clicked() {
                            let _ = open::that_detached(&r.url);
                        }
                    });
                });
                if !r.hours.is_empty() {
                    ui.label(w::text(&r.hours, 12.5, t.text_dim));
                }
                if !r.posted.is_empty() {
                    ui.label(w::text(&r.posted, 12.0, t.text_faint));
                }
            });
        });
        if !r.text.is_empty() {
            ui.add_space(8.0);
            ui.add(egui::Label::new(w::text(&r.text, 13.5, t.text)).wrap());
        }
        if !r.helpful.is_empty() {
            ui.add_space(6.0);
            ui.label(w::text(&r.helpful, 12.0, t.text_faint));
        }
    });
}

fn skeleton_rows(ui: &mut Ui, t: &Theme, height: f32) {
    ui.add_space(10.0);
    for _ in 0..5 {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        w::skeleton(ui, t, r.shrink2(vec2(0.0, 4.0)), t.corner_small());
    }
}

/// Steam hides the list (private profile), or it couldn't load.
fn hidden(ui: &mut Ui, t: &Theme, error: &str, title: &str) {
    let body = if error == "private" { t!("Its owner has chosen to keep it private.").to_string() } else { error.to_string() };
    ui.add_space(10.0);
    w::empty(ui, t, icons::LOCK_SIMPLE, title, &body);
}
