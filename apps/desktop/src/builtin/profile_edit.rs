//! Edit your profile in the app, like Steam's Edit Profile pages: name,
//! real name, custom URL, location and summary; avatar (upload, animated
//! avatars, frames); profile and mini profile backgrounds; theme; featured
//! badge; and privacy settings.
//!
//! Adapted from upstream SteamInRust PR #74 / #101 by madonchik123.
//! Item, theme and badge choices are previewed first and sent to Steam with
//! Save; Cancel puts back what Steam has. Privacy settings save as you change them.

use super::themes::{self, SteamTheme};
use super::{Profile, panel};
use client::Command;
use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Stroke, Ui, pos2, vec2};
use extensions::Ctx;
use i18n::t;
use model::{ProfileEdit, ProfileItemChoice, ProfilePrivacy, ProfileSave, art};
use ui::widgets::EditMenu;
use ui::{Theme, icons, widgets as w};

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(super) enum Section {
    #[default]
    General,
    Avatar,
    Background,
    MiniProfile,
    Theme,
    Badge,
    Privacy,
}

impl Section {
    pub fn from_arg(s: &str) -> Section {
        match s {
            "avatar" => Section::Avatar,
            "background" => Section::Background,
            "miniprofile" => Section::MiniProfile,
            "theme" => Section::Theme,
            "badge" | "favoritebadge" => Section::Badge,
            "privacy" | "settings" => Section::Privacy,
            _ => Section::General,
        }
    }
}

#[derive(Default)]
pub(super) struct EditState {
    pub section: Section,
    /// What's being edited (a copy of Steam's values until saved).
    draft: Option<ProfileEdit>,
    /// Whether Steam's current values have been asked for.
    asked: bool,
    /// Waiting for a fresh `ProfileEdit` after saving so stale state isn't re-cloned.
    reloading: bool,
    /// Saves sent and not answered yet.
    pending: usize,
    /// Sent once the saves before them went through (e.g. a background's
    /// "Full Screen" flag needs the background equipped first).
    after: Vec<Command>,
    /// Location lists asked for.
    asked_locations: std::collections::HashSet<(Option<String>, Option<String>)>,
    /// Filter text for background / mini-profile lists.
    filter: String,
    /// "See all" toggled on animated avatars and avatar frames.
    see_all: [bool; 2],
    /// Whether badges for the Featured Badge picker were requested.
    badges_asked: bool,
}

impl EditState {
    fn saving(&self) -> bool {
        self.pending > 0
    }
}

fn lang() -> String {
    i18n::language().steam_name().to_string()
}

fn original(ctx: &Ctx) -> Option<ProfileEdit> {
    ctx.state.profile_edit.as_ref().and_then(|r| r.as_ref().ok()).cloned()
}

impl Profile {
    pub(super) fn edit_page(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let me = ctx.state.steam_id;
        if !self.edit.asked {
            self.edit.asked = true;
            ctx.send(Command::LoadProfileEdit { language: lang() });
        }
        if self.edit.draft.is_none() && !self.edit.reloading {
            if let Some(Ok(e)) = &ctx.state.profile_edit {
                self.edit.draft = Some(e.clone());
            }
        }
        ui.horizontal(|ui| {
            if w::button(ui, t, Some(icons::ARROW_LEFT), t!("Back to profile"), w::Kind::Ghost, w::Size::Sm).clicked() {
                ctx.navigate("profile", None);
            }
            ui.add_space(6.0);
            ui.label(w::text_bold(t!("Edit profile"), 22.0, t.text));
        });
        ui.add_space(12.0);
        if let (None, Some(Err(e))) = (&self.edit.draft, &ctx.state.profile_edit) {
            panel(ui, t, |ui| {
                w::empty(ui, t, icons::WARNING, t!("Couldn't load your profile settings"), &i18n::tr_dyn(e));
                ui.add_space(10.0);
                ui.vertical_centered(|ui| {
                    if w::button(ui, t, Some(icons::REFRESH), t!("Retry"), w::Kind::Secondary, w::Size::Sm).clicked() {
                        self.edit.asked = false;
                    }
                });
            });
            return;
        }
        let side = 220.0;
        let preview = Preview::of(ctx, self.edit.draft.as_ref());
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(side, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(side);
                panel(ui, t, |ui| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
                        if !w::paint_cover(ui, r, &preview.avatar, CornerRadius::same(4)) {
                            w::skeleton(ui, t, r, CornerRadius::same(4));
                        }
                        if let Some(f) = &preview.frame {
                            w::paint_cover(ui, r, f, CornerRadius::ZERO);
                        }
                        ui.add(egui::Label::new(w::text_semibold(&preview.name, 14.0, t.text)).truncate());
                    });
                    ui.add_space(10.0);
                    w::menu_spacing(ui);
                    for (s, icon, label) in [
                        (Section::General, icons::USER_CIRCLE, t!("General")),
                        (Section::Avatar, icons::IMAGE, t!("Avatar")),
                        (Section::Background, icons::IMAGE, t!("Profile background")),
                        (Section::MiniProfile, icons::IMAGE, t!("Mini profile")),
                        (Section::Theme, icons::PALETTE, t!("Theme")),
                        (Section::Badge, icons::MEDAL, t!("Featured badge")),
                    ] {
                        let on = self.edit.section == s;
                        if w::menu_item_tinted(ui, t, icon, label, on.then_some(t.accent)).clicked() {
                            if self.edit.section != s {
                                self.edit.filter.clear();
                            }
                            self.edit.section = s;
                        }
                    }
                    w::menu_separator(ui, t);
                    let priv_on = self.edit.section == Section::Privacy;
                    if w::menu_item_tinted(ui, t, icons::EYE_SLASH, t!("Privacy settings"), priv_on.then_some(t.accent)).clicked() {
                        self.edit.section = Section::Privacy;
                    }
                    if w::menu_item(ui, t, icons::STOREFRONT, t!("Points Shop")).clicked() {
                        crate::links::open_http("https://store.steampowered.com/points/shop");
                    }
                });
            });
            ui.add_space(16.0);
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(ui.available_width());
                panel(ui, t, |ui| {
                    if self.edit.draft.is_none() {
                        for _ in 0..5 {
                            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
                            w::skeleton(ui, t, r.shrink2(vec2(0.0, 5.0)), t.corner_small());
                        }
                        return;
                    }
                    match self.edit.section {
                        Section::General => self.edit_general(ui, t, ctx),
                        Section::Avatar => self.edit_avatar(ui, t, ctx, me),
                        Section::Background => self.edit_background(ui, t, ctx, &preview),
                        Section::MiniProfile => self.edit_mini(ui, t, ctx, &preview),
                        Section::Theme => self.edit_theme(ui, t, ctx, &preview),
                        Section::Badge => self.edit_badge(ui, t, ctx, me),
                        Section::Privacy => self.edit_privacy(ui, t, ctx),
                    }
                });
            });
        });
    }

    fn send_saves(&mut self, ctx: &mut Ctx, now: Vec<Command>, after: Vec<Command>) {
        self.edit.pending += now.len();
        for c in now {
            ctx.send(c);
        }
        self.edit.after.extend(after);
    }

    fn edit_general(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let saving = self.edit.saving();
        let Some(d) = self.edit.draft.as_mut() else { return };
        section_title(ui, t, t!("About"), t!("Set your profile name and details. Providing additional information like your real name can help friends find you on the Steam Community."));
        let field = |ui: &mut Ui, label: &str, note: &str, value: &mut String, hint: &str| {
            ui.label(w::text_semibold(label, 13.0, t.text_dim));
            if !note.is_empty() {
                ui.add(egui::Label::new(w::text(note, 12.0, t.text_faint)).wrap());
            }
            ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(ui.available_width().min(520.0)).margin(vec2(10.0, 8.0))).with_edit_menu();
            ui.add_space(10.0);
        };
        field(ui, &format!("{} *", t!("Profile name")), t!("The name everyone sees on your profile and in chat."), &mut d.persona_name, "");
        field(ui, t!("Real name"), "", &mut d.real_name, t!("Optional"));
        ui.label(w::text_semibold(t!("Custom URL"), 13.0, t.text_dim));
        ui.horizontal_wrapped(|ui| {
            ui.label(w::text("steamcommunity.com/id/", 13.0, t.text_faint));
            ui.add(egui::TextEdit::singleline(&mut d.custom_url).desired_width(240.0).margin(vec2(10.0, 8.0))).with_edit_menu();
        });
        let slug = d.custom_url.trim();
        if !slug.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(w::text(t!("Your profile will be available at:"), 12.0, t.text_faint));
                let link = format!("https://steamcommunity.com/id/{slug}/");
                if ui.link(w::text(&link, 12.0, t.accent)).clicked() {
                    crate::links::open_http(&link);
                }
            });
        }
        ui.add_space(14.0);

        // Location: country, then its state, then the state's city.
        section_title(ui, t, t!("Location"), "");
        let countries = ctx.state.locations.get(&(None, None)).cloned();
        let states = (!d.country.is_empty()).then(|| ctx.state.locations.get(&(Some(d.country.clone()), None)).cloned()).flatten();
        let cities = (!d.country.is_empty() && !d.state.is_empty())
            .then(|| ctx.state.locations.get(&(Some(d.country.clone()), Some(d.state.clone()))).cloned())
            .flatten();
        let mut want = vec![(None, None)];
        if !d.country.is_empty() {
            want.push((Some(d.country.clone()), None));
        }
        if !d.country.is_empty() && !d.state.is_empty() {
            want.push((Some(d.country.clone()), Some(d.state.clone())));
        }
        for key in want {
            if self.edit.asked_locations.insert(key.clone()) {
                ctx.send(Command::LoadLocations { country: key.0, state: key.1 });
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 8.0);
            if pick(ui, t, "loc-country", t!("Country"), &countries, &mut d.country) {
                d.state.clear();
                d.city.clear();
            }
            if states.as_ref().is_some_and(|s| !s.is_empty()) && pick(ui, t, "loc-state", t!("State or region"), &states, &mut d.state) {
                d.city.clear();
            }
            if cities.as_ref().is_some_and(|c| !c.is_empty()) {
                pick(ui, t, "loc-city", t!("City"), &cities, &mut d.city);
            }
        });
        ui.add_space(14.0);

        section_title(ui, t, t!("Summary"), t!("A few words about you, shown under your name."));
        let summary_w = ui.available_width();
        egui::Frame::new()
            .fill(t.raised)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(t.corner_small())
            .show(ui, |ui| {
                ui.set_width(summary_w);
                egui::ScrollArea::vertical().id_salt("profile-edit-summary").max_height(190.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut d.summary)
                            .desired_rows(5)
                            .desired_width(summary_w)
                            .frame(egui::Frame::NONE)
                            .margin(vec2(10.0, 8.0)),
                    )
                    .with_edit_menu();
                });
            });
        ui.add_space(10.0);
        w::check(ui, t, &mut d.hide_awards, w::text(t!("Hide the awards you've been given on your profile"), 13.5, t.text));
        ui.add_space(14.0);

        let orig = original(ctx);
        let changed = orig.as_ref().is_some_and(|orig| {
            (&orig.persona_name, &orig.real_name, &orig.custom_url, &orig.country, &orig.state, &orig.city, &orig.summary, orig.hide_awards)
                != (&d.persona_name, &d.real_name, &d.custom_url, &d.country, &d.state, &d.city, &d.summary, d.hide_awards)
        });
        let (save, cancel) = save_bar(ui, t, changed, saving, !d.persona_name.trim().is_empty());
        if save {
            let cmd = Command::SaveProfileInfo(Box::new(d.clone()));
            self.send_saves(ctx, vec![cmd], vec![]);
        }
        if cancel {
            self.edit.draft = orig;
        }
    }

    fn edit_avatar(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, me_id: model::SteamId) {
        let me = ctx.state.persona(me_id);
        let saving = self.edit.saving();
        let still = art::avatar_full(me.avatar_hash.as_deref());
        let Some(d) = self.edit.draft.as_mut() else { return };
        let animated = d.animated_avatars.iter().find(|i| i.id == d.equipped[3]).map(|i| i.image.clone());
        let frame = d.frames.iter().find(|i| i.id == d.equipped[2]).map(|i| i.image.clone());
        let shown = animated.unwrap_or_else(|| still.clone());

        section_title(ui, t, t!("Avatar"), t!("Choose a picture for your profile. It's shown at 184×184."));
        let mut upload = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            for size in [128.0, 64.0, 32.0] {
                ui.vertical(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
                    avatar_with_frame(ui, t, r, &shown, frame.as_deref());
                    ui.label(w::text(format!("{size:.0} px"), 11.5, t.text_faint));
                });
            }
            ui.vertical(|ui| {
                if saving {
                    ui.add(egui::Spinner::new().size(18.0));
                } else if w::button(ui, t, Some(icons::IMAGE), t!("Upload your avatar"), w::Kind::Primary, w::Size::Md).clicked() {
                    upload = rfd::FileDialog::new().add_filter(t!("Images").to_string(), &["jpg", "jpeg", "png"]).pick_file();
                }
                ui.add_space(4.0);
                ui.add(egui::Label::new(w::text(t!("JPEG or PNG. Steam crops it to a square. An upload is saved right away."), 12.0, t.text_faint)).wrap());
            });
        });
        if let Some(path) = upload {
            self.edit.pending += 1;
            ctx.send(Command::UploadAvatar { path });
        }

        let tile = vec2(88.0, 88.0);
        let per_row = (((ui.available_width() + 10.0) / (tile.x + 10.0)).floor() as usize).max(1);
        ui.add_space(18.0);
        section_header_more(ui, t, t!("Animated avatar"), t!("Animated avatars you own (from the Points Shop)."), &mut self.edit.see_all[0], d.animated_avatars.len() + 1 > per_row);
        if let Some(id) = item_grid(
            ui,
            t,
            &d.animated_avatars,
            d.equipped[3],
            "",
            tile,
            false,
            self.edit.see_all[0],
            &|ui, r, it| {
                if !w::paint_contain(ui, r.shrink(4.0), &it.image, Align2::CENTER_CENTER) {
                    w::skeleton(ui, t, r.shrink(4.0), t.corner_small());
                }
            },
            &|ui, r| {
                if !w::paint_cover(ui, r.shrink(4.0), &still, t.corner_small()) {
                    ui.painter().rect_filled(r.shrink(4.0), t.corner_small(), t.raised);
                }
            },
        ) {
            d.equipped[3] = id;
        }

        ui.add_space(18.0);
        section_header_more(ui, t, t!("Avatar frame"), t!("Frames you own go around your avatar everywhere on Steam."), &mut self.edit.see_all[1], d.frames.len() + 1 > per_row);
        if let Some(id) = item_grid(
            ui,
            t,
            &d.frames,
            d.equipped[2],
            "",
            tile,
            false,
            self.edit.see_all[1],
            &|ui, r, it| {
                avatar_with_frame(ui, t, r.shrink(4.0), &shown, Some(&it.image));
            },
            &|ui, r| {
                avatar_with_frame(ui, t, r.shrink(4.0), &shown, None);
            },
        ) {
            d.equipped[2] = id;
        }

        ui.add_space(14.0);
        let orig = original(ctx);
        let o = orig.as_ref().map(|o| o.equipped).unwrap_or_default();
        let changed = o[2] != d.equipped[2] || o[3] != d.equipped[3];
        let (save, cancel) = save_bar(ui, t, changed, saving, true);
        let now: Vec<Command> = [3usize, 2].into_iter().filter(|k| o[*k] != d.equipped[*k]).map(|k| Command::EquipProfileItem { kind: k as u8, id: d.equipped[k] }).collect();
        if cancel {
            d.equipped[2] = o[2];
            d.equipped[3] = o[3];
        }
        if save {
            self.send_saves(ctx, now, vec![]);
        }
    }

    fn edit_background(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, preview: &Preview) {
        let saving = self.edit.saving();
        let filter = &mut self.edit.filter;
        let Some(d) = self.edit.draft.as_mut() else { return };
        section_title(ui, t, t!("Profile background"), t!("Backgrounds you own, shown behind your profile."));
        let chosen = d.backgrounds.iter().find(|i| i.id == d.equipped[0]).cloned();
        let width = ui.available_width();
        let (r, _) = ui.allocate_exact_size(vec2(width, (width * 0.36).clamp(180.0, 290.0)), Sense::hover());
        profile_preview(ui, t, r, chosen.as_ref(), d.background_full, themes::get(&d.theme), preview);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_enabled_ui(chosen.is_some(), |ui| {
                if w::chip(ui, t, t!("Full screen"), d.background_full).clicked() {
                    d.background_full = true;
                }
                if w::chip(ui, t, t!("Original size"), !d.background_full).clicked() {
                    d.background_full = false;
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                w::search(ui, t, filter, t!("Filter"), 240.0);
            });
        });
        ui.add_space(10.0);
        let tile = vec2(192.0, 108.0);
        if let Some(id) = item_grid(
            ui,
            t,
            &d.backgrounds,
            d.equipped[0],
            filter,
            tile,
            true,
            true,
            &|ui, r, it| {
                let src = if it.large.is_empty() { &it.image } else { &it.large };
                if !w::paint_cover(ui, r, src, t.corner_small()) {
                    w::skeleton(ui, t, r, t.corner_small());
                }
                if !it.movie.is_empty() {
                    animated_mark(ui, t, r);
                }
            },
            &|ui, r| {
                ui.painter().rect_filled(r, t.corner_small(), t.raised);
            },
        ) {
            d.equipped[0] = id;
        }
        ui.add_space(14.0);
        let orig = original(ctx);
        let (o, o_full) = orig.as_ref().map(|o| (o.equipped[0], o.background_full)).unwrap_or_default();
        let changed = o != d.equipped[0] || (d.equipped[0] != 0 && o_full != d.background_full);
        let (save, cancel) = save_bar(ui, t, changed, saving, true);
        if cancel {
            d.equipped[0] = o;
            d.background_full = o_full;
        }
        if save {
            let (id, full) = (d.equipped[0], d.background_full);
            let flags = (id != 0 && (o != id || o_full != full)).then_some(Command::SetBackgroundFullScreen { id, full });
            if o != id {
                self.send_saves(ctx, vec![Command::EquipProfileItem { kind: 0, id }], flags.into_iter().collect());
            } else {
                self.send_saves(ctx, flags.into_iter().collect(), vec![]);
            }
        }
    }

    fn edit_mini(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, preview: &Preview) {
        let saving = self.edit.saving();
        let filter = &mut self.edit.filter;
        let Some(d) = self.edit.draft.as_mut() else { return };
        section_title(ui, t, t!("Mini profile"), t!("The background people see when they hover over your name."));
        let chosen = d.mini_backgrounds.iter().find(|i| i.id == d.equipped[1]).cloned();
        let card_w = ui.available_width().min(360.0);
        let (r, _) = ui.allocate_exact_size(vec2(card_w, 210.0), Sense::hover());
        mini_preview(ui, t, r, chosen.as_ref(), preview);
        ui.add_space(10.0);
        w::search(ui, t, filter, t!("Filter"), 240.0);
        ui.add_space(10.0);
        let tile = vec2(192.0, 108.0);
        if let Some(id) = item_grid(
            ui,
            t,
            &d.mini_backgrounds,
            d.equipped[1],
            filter,
            tile,
            true,
            true,
            &|ui, r, it| {
                let src = if it.large.is_empty() { &it.image } else { &it.large };
                if !w::paint_cover(ui, r, src, t.corner_small()) {
                    w::skeleton(ui, t, r, t.corner_small());
                }
                if !it.movie.is_empty() {
                    animated_mark(ui, t, r);
                }
            },
            &|ui, r| {
                ui.painter().rect_filled(r, t.corner_small(), t.raised);
            },
        ) {
            d.equipped[1] = id;
        }
        ui.add_space(14.0);
        let o = original(ctx).map(|o| o.equipped[1]).unwrap_or_default();
        let (save, cancel) = save_bar(ui, t, o != d.equipped[1], saving, true);
        if cancel {
            d.equipped[1] = o;
        }
        if save {
            let cmd = Command::EquipProfileItem { kind: 1, id: d.equipped[1] };
            self.send_saves(ctx, vec![cmd], vec![]);
        }
    }

    fn edit_theme(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, preview: &Preview) {
        let saving = self.edit.saving();
        let Some(d) = self.edit.draft.as_mut() else { return };
        section_title(ui, t, t!("Theme"), t!("The colors of your profile page on Steam."));
        let background = d.backgrounds.iter().find(|i| i.id == d.equipped[0]).cloned();
        let width = ui.available_width();
        let (r, _) = ui.allocate_exact_size(vec2(width, (width * 0.36).clamp(180.0, 290.0)), Sense::hover());
        profile_preview(ui, t, r, background.as_ref(), d.background_full, themes::get(&d.theme), preview);
        ui.add_space(14.0);

        if d.themes.is_empty() {
            ui.label(w::text(t!("No themes available."), 13.0, t.text_faint));
        } else {
            let mut list: Vec<(String, String)> = vec![(String::new(), String::new())];
            for (id, title) in &d.themes {
                if !id.is_empty() && !id.eq_ignore_ascii_case("Default") {
                    list.push((id.clone(), title.clone()));
                }
            }
            let (known, extra): (Vec<_>, Vec<_>) = list.into_iter().partition(|(id, _)| themes::get(id).is_some());
            let cols = if ui.available_width() < 420.0 { 2 } else { 3 };
            let cell = (ui.available_width() / cols as f32).floor();
            for row in known.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (id, title) in row {
                        let Some(colors) = themes::get(id) else { continue };
                        let on = id == &d.theme || (id.is_empty() && d.theme.eq_ignore_ascii_case("Default"));
                        if swatch(ui, t, &themes::name(id, title), colors, on, cell) {
                            d.theme = id.clone();
                        }
                    }
                });
                ui.add_space(10.0);
            }
            if !extra.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    for (id, title) in &extra {
                        let label = themes::name(id, title);
                        if w::chip(ui, t, &label, *id == d.theme).clicked() {
                            d.theme = id.clone();
                        }
                    }
                });
            }
        }
        ui.add_space(14.0);
        let o = original(ctx).map(|o| o.theme).unwrap_or_default();
        let changed = o != d.theme && !(o.is_empty() && d.theme.eq_ignore_ascii_case("Default")) && !(d.theme.is_empty() && o.eq_ignore_ascii_case("Default"));
        let (save, cancel) = save_bar(ui, t, changed, saving, true);
        if cancel {
            d.theme = o;
        }
        if save {
            let cmd = Command::SetProfileTheme(d.theme.clone());
            self.send_saves(ctx, vec![cmd], vec![]);
        }
    }

    fn edit_badge(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, me: model::SteamId) {
        let saving = self.edit.saving();
        let badges = ctx.state.badges.get(&me).cloned();
        if badges.is_none() && !self.edit.badges_asked {
            self.edit.badges_asked = true;
            ctx.send(Command::LoadBadges { steam_id: me });
        }
        let Some(d) = self.edit.draft.as_mut() else { return };
        section_title(
            ui,
            t,
            t!("Featured badge"),
            t!("The badge shown next to your level on your profile and mini profile. Community badges can be featured here; game badges from Steam's own page."),
        );
        match &badges {
            None => {
                for _ in 0..2 {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 80.0), Sense::hover());
                    w::skeleton(ui, t, r.shrink2(vec2(0.0, 4.0)), t.corner_small());
                }
            }
            Some(Err(e)) => {
                ui.label(w::text(i18n::tr_dyn(e), 13.0, t.text_dim));
            }
            Some(Ok(b)) => {
                let list: Vec<(u32, &model::Badge)> = b.items.iter().filter_map(|x| community_badge_id(&x.url).map(|id| (id, x))).collect();
                if list.is_empty() {
                    ui.label(w::text(t!("No badges yet."), 13.5, t.text_faint));
                } else {
                    let tile = vec2(124.0, 124.0);
                    let gap = 10.0;
                    let cols = (((ui.available_width() + gap) / (tile.x + gap)).floor() as usize).max(1);
                    let mut options: Vec<(u32, Option<&model::Badge>)> = vec![(0, None)];
                    options.extend(list.iter().map(|(id, b)| (*id, Some(*b))));
                    for row in options.chunks(cols) {
                        let subtitle = row.iter().any(|(_, badge)| badge.is_some_and(|b| !b.detail.is_empty()));
                        if w::skip_offscreen_row(ui, tile.y + 22.0 + if subtitle { 15.0 } else { 0.0 }) {
                            ui.add_space(6.0);
                            continue;
                        }
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            for (id, badge) in row {
                                let label = badge.map(|b| b.title.as_str()).unwrap_or(t!("None"));
                                let sub = badge.map(|b| b.detail.as_str()).unwrap_or("");
                                if tile_button(ui, t, tile, label, sub, *id == d.favorite_badge, |ui, r| match badge.and_then(|b| b.icon.as_deref()) {
                                    Some(u) => {
                                        if !w::paint_contain(ui, r.shrink(10.0), u, Align2::CENTER_CENTER) {
                                            w::skeleton(ui, t, r.shrink(10.0), t.corner_small());
                                        }
                                    }
                                    None => {
                                        ui.painter().rect_filled(r, t.corner_small(), t.raised);
                                        if badge.is_some() {
                                            ui.painter().text(r.center(), Align2::CENTER_CENTER, icons::MEDAL, w::font(28.0), t.accent);
                                        }
                                    }
                                }) {
                                    d.favorite_badge = *id;
                                }
                            }
                        });
                        ui.add_space(6.0);
                    }
                }
            }
        }
        ui.add_space(14.0);
        let o = original(ctx).map(|o| o.favorite_badge).unwrap_or_default();
        let (save, cancel) = save_bar(ui, t, o != d.favorite_badge, saving, true);
        if cancel {
            d.favorite_badge = o;
        }
        if save {
            let cmd = Command::SetFavoriteBadge { badge: d.favorite_badge };
            self.send_saves(ctx, vec![cmd], vec![]);
        }
    }

    fn edit_privacy(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let Some(d) = self.edit.draft.as_mut() else { return };
        section_title(ui, t, t!("Privacy settings"), t!("Choose who can see your profile and its parts. Changes are saved right away."));
        let before = d.privacy;
        privacy_rows(ui, t, &mut d.privacy);
        if d.privacy != before {
            ctx.send(Command::SaveProfilePrivacy(d.privacy));
        }
    }

    pub(super) fn edit_event(&mut self, event: &client::Event, ctx: &mut Ctx) {
        if let client::Event::ProfileEdit(res) = event {
            if self.edit.reloading || self.edit.draft.is_none() {
                self.edit.reloading = false;
                if let Ok(e) = res {
                    self.edit.draft = Some(e.clone());
                }
            }
            return;
        }
        let client::Event::ProfileSaved { what, result } = event else { return };
        if *what == ProfileSave::Privacy && self.edit.pending == 0 {
            match result {
                Ok(()) => ctx.toast(t!("Privacy settings saved")),
                Err(e) => {
                    ctx.toast(t!("Steam didn't save it: {error}", error = i18n::tr_dyn(e)));
                    self.edit.reloading = true;
                    ctx.send(Command::LoadProfileEdit { language: lang() });
                }
            }
            return;
        }
        self.edit.pending = self.edit.pending.saturating_sub(1);
        match result {
            Ok(()) => {
                let after = std::mem::take(&mut self.edit.after);
                self.edit.pending += after.len();
                for c in after {
                    ctx.send(c);
                }
                if self.edit.pending > 0 {
                    return;
                }
                ctx.toast(match what {
                    ProfileSave::Info => t!("Profile saved"),
                    ProfileSave::Privacy => t!("Privacy settings saved"),
                    ProfileSave::Avatar => t!("Avatar changed"),
                    ProfileSave::Item | ProfileSave::BackgroundFlags | ProfileSave::Theme | ProfileSave::Badge => t!("Profile updated"),
                });
                self.edit.reloading = true;
                ctx.send(Command::LoadProfileEdit { language: lang() });
                ctx.send(Command::LoadProfile { steam_id: ctx.state.steam_id });
                self.requested.remove(&ctx.state.steam_id);
            }
            Err(e) => {
                self.edit.after.clear();
                ctx.toast(t!("Steam didn't save it: {error}", error = i18n::tr_dyn(e)));
            }
        }
    }
}

fn privacy_rows(ui: &mut Ui, t: &Theme, p: &mut ProfilePrivacy) {
    let opts = [(3u8, t!("Public")), (2u8, t!("Friends only")), (1u8, t!("Private"))];
    let row = |ui: &mut Ui, label: &str, help: &str, value: &mut u8, max: u8| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - 340.0).max(200.0));
                ui.label(w::text_semibold(label, 14.0, t.text));
                if !help.is_empty() {
                    ui.add(egui::Label::new(w::text(help, 12.0, t.text_faint)).wrap());
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let list: Vec<(u8, &str)> = opts.iter().filter(|(v, _)| *v <= max).map(|(v, l)| (*v, &**l)).collect();
                *value = (*value).min(max);
                w::segmented(ui, t, value, &list);
            });
        });
        ui.add_space(10.0);
    };
    row(ui, t!("My profile"), t!("Your level, badges, friends, groups and summary."), &mut p.profile, 3);
    let top = p.profile;
    row(ui, t!("Game details"), t!("The games you own, your playtime and achievements."), &mut p.games, top);
    let mut hide_time = p.playtime == 1;
    if p.games > 1 && w::check(ui, t, &mut hide_time, w::text(t!("Always keep my total playtime private"), 13.5, t.text)).changed() {
        p.playtime = if hide_time { 1 } else { p.games };
    }
    if !hide_time {
        p.playtime = p.games;
    }
    ui.add_space(6.0);
    row(ui, t!("Friends list"), t!("Who can see your friends."), &mut p.friends, top);
    row(ui, t!("Inventory"), t!("Your items, trading cards and gifts."), &mut p.inventory, top);
    let mut hide_gifts = p.gifts == 1;
    if p.inventory > 1 && w::check(ui, t, &mut hide_gifts, w::text(t!("Always keep Steam Gifts private"), 13.5, t.text)).changed() {
        p.gifts = if hide_gifts { 1 } else { p.inventory };
    }
    if !hide_gifts {
        p.gifts = p.inventory;
    }
    ui.add_space(6.0);
    // Comments: 1 everyone, 0 friends, 2 nobody.
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 340.0).max(200.0));
            ui.label(w::text_semibold(t!("Can post comments on my profile"), 14.0, t.text));
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            w::segmented(ui, t, &mut p.comments, &[(1, t!("Public")), (0, t!("Friends only")), (2, t!("Private"))]);
        });
    });
}

fn section_title(ui: &mut Ui, t: &Theme, title: &str, body: &str) {
    ui.label(w::text_bold(title, 17.0, t.text));
    if !body.is_empty() {
        ui.add(egui::Label::new(w::text(body, 12.5, t.text_dim)).wrap());
    }
    ui.add_space(10.0);
}

fn section_header_more(ui: &mut Ui, t: &Theme, title: &str, body: &str, all: &mut bool, more: bool) {
    ui.horizontal(|ui| {
        ui.label(w::text_bold(title, 17.0, t.text));
        if more {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.link(w::text(if *all { t!("Show less") } else { t!("See all") }, 12.5, t.accent)).clicked() {
                    *all = !*all;
                }
            });
        }
    });
    if !body.is_empty() {
        ui.add(egui::Label::new(w::text(body, 12.5, t.text_dim)).wrap());
    }
    ui.add_space(10.0);
}

fn save_bar(ui: &mut Ui, t: &Theme, changed: bool, saving: bool, valid: bool) -> (bool, bool) {
    let (mut save, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if saving {
            ui.add(egui::Spinner::new().size(18.0));
        } else if ui.add_enabled_ui(changed && valid, |ui| w::button(ui, t, Some(icons::CHECK), t!("Save"), w::Kind::Primary, w::Size::Md)).inner.clicked() {
            save = true;
        }
        if ui.add_enabled_ui(changed && !saving, |ui| w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Md)).inner.clicked() {
            cancel = true;
        }
        if changed && !saving {
            ui.label(w::text(t!("Preview: not saved yet"), 12.5, t.away));
        }
    });
    (save, cancel)
}

/// A dropdown of (code, name); true if the choice changed.
fn pick(ui: &mut Ui, t: &Theme, id: &str, label: &str, list: &Option<Vec<(String, String)>>, value: &mut String) -> bool {
    let shown = list
        .as_ref()
        .and_then(|l| l.iter().find(|(c, _)| c == value))
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| if value.is_empty() { t!("Not set").to_string() } else { value.clone() });
    let mut changed = false;
    ui.vertical(|ui| {
        ui.label(w::text_semibold(label, 12.5, t.text_dim));
        egui::ComboBox::from_id_salt(id).selected_text(shown).width(220.0).height(360.0).show_ui(ui, |ui| {
            if ui.selectable_label(value.is_empty(), t!("Not set")).clicked() && !value.is_empty() {
                value.clear();
                changed = true;
            }
            for (code, name) in list.iter().flatten() {
                if ui.selectable_label(code == value, name).clicked() && code != value {
                    *value = code.clone();
                    changed = true;
                }
            }
        });
    });
    changed
}

/// A community badge's id from its link (`.../badges/2`); game badges have none.
fn community_badge_id(url: &str) -> Option<u32> {
    let rest = url.split("/badges/").nth(1)?;
    rest.trim_end_matches('/').split(['/', '?']).next()?.parse().ok()
}

fn tile_button(ui: &mut Ui, t: &Theme, size: egui::Vec2, name: &str, sub: &str, on: bool, pic: impl FnOnce(&Ui, Rect)) -> bool {
    let line = 16.0;
    let small = if sub.is_empty() { 0.0 } else { 15.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(size.x, size.y + 6.0 + line + small), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp.clicked();
    }
    let img = Rect::from_min_size(rect.min, size);
    pic(ui, img);
    let hover = resp.hovered();
    let stroke = if on {
        Stroke::new(2.0, t.accent)
    } else if hover {
        Stroke::new(1.0, t.text_faint)
    } else {
        Stroke::new(1.0, t.border)
    };
    ui.painter().rect_stroke(img, t.corner_small(), stroke, egui::StrokeKind::Inside);
    if on {
        let c = pos2(img.right() - 12.0, img.top() + 12.0);
        ui.painter().circle_filled(c, 9.0, t.accent);
        ui.painter().text(c, Align2::CENTER_CENTER, icons::CHECK, w::font(11.0), t.on_accent);
    }
    let draw_line = |s: &str, font: egui::FontId, color: Color32, y: f32| {
        let mut job = egui::text::LayoutJob::simple_singleline(s.to_string(), font, color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(size.x);
        let g = ui.painter().layout_job(job);
        ui.painter().galley(pos2(img.left(), y), g, color);
    };
    draw_line(name, w::font_medium(12.5), if on { t.accent } else { t.text }, img.bottom() + 4.0);
    if !sub.is_empty() {
        draw_line(sub, w::font(11.5), t.text_faint, img.bottom() + 4.0 + line);
    }
    let tip = if sub.is_empty() { name.to_string() } else { format!("{name}\n{sub}") };
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

#[allow(clippy::too_many_arguments)]
fn item_grid(
    ui: &mut Ui,
    t: &Theme,
    items: &[ProfileItemChoice],
    chosen: u64,
    filter: &str,
    tile: egui::Vec2,
    with_game: bool,
    all: bool,
    pic: &dyn Fn(&Ui, Rect, &ProfileItemChoice),
    none_pic: &dyn Fn(&Ui, Rect),
) -> Option<u64> {
    if items.is_empty() {
        ui.label(w::text(t!("You don't own any yet. They're in the Points Shop."), 13.0, t.text_faint));
        if ui.link(w::text(t!("Open the Points Shop"), 12.5, t.accent)).clicked() {
            crate::links::open_http("https://store.steampowered.com/points/shop");
        }
        return None;
    }
    let f = filter.trim().to_lowercase();
    let shown: Vec<&ProfileItemChoice> = items
        .iter()
        .filter(|i| f.is_empty() || i.name.to_lowercase().contains(&f) || i.app_name.to_lowercase().contains(&f))
        .collect();
    let gap = 10.0;
    let cols = (((ui.available_width() + gap) / (tile.x + gap)).floor() as usize).max(1);
    let mut cells: Vec<Option<&ProfileItemChoice>> = if f.is_empty() { vec![None] } else { Vec::new() };
    cells.extend(shown.iter().copied().map(Some));
    if !all {
        cells.truncate(cols);
    }
    if cells.is_empty() {
        ui.label(w::text(t!("Nothing matches the filter."), 13.0, t.text_faint));
        return None;
    }
    let mut picked = None;
    for row in cells.chunks(cols) {
        let subtitle = with_game && row.iter().any(|cell| cell.is_some_and(|item| !item.app_name.is_empty()));
        if w::skip_offscreen_row(ui, tile.y + 22.0 + if subtitle { 15.0 } else { 0.0 }) {
            ui.add_space(6.0);
            continue;
        }
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for cell in row {
                let clicked = match cell {
                    None => tile_button(ui, t, tile, t!("None"), "", chosen == 0, |ui, r| none_pic(ui, r)),
                    Some(it) => {
                        let game = if with_game { it.app_name.as_str() } else { "" };
                        tile_button(ui, t, tile, &it.name, game, it.id == chosen, |ui, r| pic(ui, r, it))
                    }
                };
                if clicked {
                    picked = Some(cell.map(|i| i.id).unwrap_or(0));
                }
            }
        });
        ui.add_space(6.0);
    }
    picked
}

fn animated_mark(ui: &Ui, t: &Theme, r: Rect) {
    let c = pos2(r.right() - 14.0, r.top() + 14.0);
    ui.painter().circle_filled(c, 9.0, Color32::from_black_alpha(175));
    ui.painter().text(c, Align2::CENTER_CENTER, icons::PLAY, w::font(10.0), t.text);
}

fn avatar_with_frame(ui: &Ui, t: &Theme, r: Rect, avatar: &str, frame: Option<&str>) {
    let inner = if frame.is_some() { r.shrink(r.width() * 0.09) } else { r };
    if !w::paint_cover(ui, inner, avatar, CornerRadius::same(4)) {
        w::skeleton(ui, t, inner, CornerRadius::same(4));
    }
    match frame {
        Some(f) => {
            w::paint_cover(ui, r, f, CornerRadius::ZERO);
        }
        None => {
            ui.painter().rect_stroke(r, CornerRadius::same(4), Stroke::new(1.5, t.border), egui::StrokeKind::Inside);
        }
    }
}

struct Preview {
    name: String,
    status: String,
    avatar: String,
    frame: Option<String>,
    level: Option<u32>,
    badge: Option<model::Badge>,
}

impl Preview {
    fn of(ctx: &Ctx, d: Option<&ProfileEdit>) -> Self {
        let me = ctx.state.persona(ctx.state.steam_id);
        let name = d
            .map(|d| d.persona_name.trim())
            .filter(|n| !n.is_empty())
            .map(String::from)
            .unwrap_or_else(|| if ctx.state.persona_name.is_empty() { me.name.clone() } else { ctx.state.persona_name.clone() });
        let still = art::avatar_full(me.avatar_hash.as_deref());
        let avatar = d
            .and_then(|d| d.animated_avatars.iter().find(|i| i.id == d.equipped[3]))
            .map(|i| i.image.clone())
            .unwrap_or(still);
        let frame = d.and_then(|d| d.frames.iter().find(|i| i.id == d.equipped[2])).map(|i| i.image.clone());
        let badges = ctx.state.badges.get(&ctx.state.steam_id).and_then(|b| b.as_ref().ok());
        let featured = d.map(|d| d.favorite_badge).unwrap_or(0);
        let badge = badges.and_then(|b| b.items.iter().find(|x| featured != 0 && community_badge_id(&x.url) == Some(featured)).cloned());
        let level = ctx.state.level.or_else(|| badges.map(|b| b.level));
        Self { name, status: crate::shell::status_text(ctx.state, &me), avatar, frame, level, badge }
    }
}

fn profile_preview(ui: &mut Ui, t: &Theme, r: Rect, bg: Option<&ProfileItemChoice>, full: bool, theme: Option<SteamTheme>, p: &Preview) {
    let painter = ui.painter_at(r);
    painter.rect_filled(r, t.corner(), t.chrome);
    let still = bg.map(|b| if b.large.is_empty() { b.image.as_str() } else { b.large.as_str() });
    if let Some(url) = still {
        let area = if full { r } else { Rect::from_center_size(r.center(), vec2(r.width() * 0.78, r.height())) };
        let child = ui.new_child(egui::UiBuilder::new().max_rect(r));
        w::paint_cover(&child, area, url, t.corner());
    }
    let panel_rect = Rect::from_center_size(pos2(r.center().x, r.center().y + r.height() * 0.05), vec2(r.width() * 0.66, r.height() * 0.82));
    painter.rect_filled(panel_rect, CornerRadius::same(4), t.surface.gamma_multiply(0.92));
    if let Some(s) = theme {
        painter.add(s.preview_overlay(panel_rect));
    }
    let pad = panel_rect.width() * 0.04;
    let av = Rect::from_min_size(panel_rect.min + vec2(pad, pad), egui::Vec2::splat(panel_rect.height() * 0.30));
    avatar_with_frame(ui, t, av, &p.avatar, p.frame.as_deref());
    let x = av.right() + pad;
    painter.text(pos2(x, av.top()), Align2::LEFT_TOP, &p.name, w::font_bold(15.0), t.text);
    painter.text(pos2(x, av.top() + 20.0), Align2::LEFT_TOP, &p.status, w::font(12.0), t.accent);
    if let Some(level) = p.level {
        let c = pos2(panel_rect.right() - pad - 15.0, av.top() + 15.0);
        painter.circle_stroke(c, 13.0, Stroke::new(1.8, t.text_dim));
        painter.text(c, Align2::CENTER_CENTER, level.to_string(), w::font_bold(11.5), t.text);
    }
    let top = av.bottom() + pad;
    let main = Rect::from_min_max(pos2(panel_rect.left() + pad, top), pos2(panel_rect.left() + panel_rect.width() * 0.68, panel_rect.bottom() - pad));
    let side = Rect::from_min_max(pos2(main.right() + pad, top), pos2(panel_rect.right() - pad, panel_rect.bottom() - pad));
    for (i, h) in [0.42f32, 0.30].iter().enumerate() {
        let y = main.top() + main.height() * if i == 0 { 0.0 } else { 0.52 };
        let section = Rect::from_min_size(pos2(main.left(), y), vec2(main.width(), main.height() * h));
        painter.rect_filled(section, t.corner_small(), Color32::from_black_alpha(70));
        let bar = Rect::from_min_size(section.min, vec2(section.width(), (section.height() * 0.28).min(16.0)));
        painter.rect_filled(bar, t.corner_small(), t.raised);
    }
    let step = side.height() / 5.5;
    for i in 0..5 {
        let y = side.top() + step * i as f32;
        painter.rect_filled(Rect::from_min_size(pos2(side.left(), y), vec2(side.width(), step * 0.7)), t.corner_small(), t.raised);
    }
    painter.rect_stroke(r, t.corner(), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
}

fn mini_preview(ui: &mut Ui, t: &Theme, r: Rect, bg: Option<&ProfileItemChoice>, p: &Preview) {
    let painter = ui.painter_at(r);
    painter.rect_filled(r, t.corner(), t.raised);
    let still = bg.map(|b| if b.large.is_empty() { b.image.as_str() } else { b.large.as_str() });
    let top = Rect::from_min_size(r.min, vec2(r.width(), r.height() * 0.58));
    if let Some(s) = still {
        let child = ui.new_child(egui::UiBuilder::new().max_rect(top));
        if w::paint_cover(&child, top, s, CornerRadius::ZERO) {
            painter.rect_filled(top, CornerRadius::ZERO, Color32::from_black_alpha(95));
        }
    }
    let pad = 12.0;
    let av = Rect::from_min_size(pos2(r.left() + pad, top.bottom() - 62.0 - pad), egui::Vec2::splat(62.0));
    avatar_with_frame(ui, t, av, &p.avatar, p.frame.as_deref());
    let x = av.right() + pad;
    painter.text(pos2(x, av.center().y - 3.0), Align2::LEFT_BOTTOM, &p.name, w::font_bold(15.0), t.text);
    painter.text(pos2(x, av.center().y + 3.0), Align2::LEFT_TOP, &p.status, w::font(12.5), t.accent);
    let y = top.bottom() + (r.bottom() - top.bottom()) / 2.0;
    if let Some(level) = p.level {
        let c = pos2(r.left() + pad + 15.0, y);
        painter.circle_stroke(c, 14.0, Stroke::new(2.0, t.text_dim));
        painter.text(c, Align2::CENTER_CENTER, level.to_string(), w::font_bold(12.0), t.text);
        painter.text(pos2(c.x + 22.0, y), Align2::LEFT_CENTER, t!("Steam Level"), w::font(12.5), t.text_dim);
    }
    if let Some(b) = &p.badge {
        let size = 36.0;
        let br = Rect::from_center_size(pos2(r.right() - pad - size / 2.0, y), egui::Vec2::splat(size));
        if let Some(u) = &b.icon {
            w::paint_contain(ui, br, u, Align2::CENTER_CENTER);
        } else {
            painter.text(br.center(), Align2::CENTER_CENTER, icons::MEDAL, w::font(22.0), t.accent);
        }
        let mut job = egui::text::LayoutJob::simple_singleline(b.title.clone(), w::font(12.0), t.text_dim);
        job.wrap = egui::text::TextWrapping::truncate_at_width(r.width() * 0.38);
        let g = ui.painter().layout_job(job);
        let at = pos2(br.left() - 8.0 - g.size().x, y - g.size().y / 2.0);
        ui.painter().galley(at, g, t.text_dim);
    }
    painter.rect_stroke(r, t.corner(), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
}

fn swatch(ui: &mut Ui, t: &Theme, name: &str, s: SteamTheme, on: bool, width: f32) -> bool {
    let d = (width * 0.56).clamp(56.0, 112.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, d + 28.0), Sense::click());
    let c = pos2(rect.center().x, rect.top() + d / 2.0 + 4.0);
    let p = ui.painter();
    p.add(s.swatch_shape(c, d / 2.0));
    let ring = if on {
        Stroke::new(3.0, t.accent)
    } else if resp.hovered() {
        Stroke::new(2.5, t.text_dim)
    } else {
        Stroke::new(2.0, t.border)
    };
    p.circle_stroke(c, d / 2.0 + 1.5, ring);
    let mut job = egui::text::LayoutJob::simple_singleline(name.to_string(), w::font_medium(12.5), if on { t.accent } else { t.text_dim });
    job.wrap = egui::text::TextWrapping::truncate_at_width(width - 8.0);
    let g = p.layout_job(job);
    p.galley(pos2(c.x - g.size().x / 2.0, rect.bottom() - g.size().y), g, t.text);
    resp.on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_edit_sections_and_badge_ids() {
        assert_eq!(Section::from_arg(""), Section::General);
        assert_eq!(Section::from_arg("avatar"), Section::Avatar);
        assert_eq!(Section::from_arg("background"), Section::Background);
        assert_eq!(Section::from_arg("miniprofile"), Section::MiniProfile);
        assert_eq!(Section::from_arg("theme"), Section::Theme);
        assert_eq!(Section::from_arg("badge"), Section::Badge);
        assert_eq!(Section::from_arg("favoritebadge"), Section::Badge);
        assert_eq!(Section::from_arg("privacy"), Section::Privacy);
        assert_eq!(Section::from_arg("settings"), Section::Privacy);

        assert_eq!(community_badge_id("https://steamcommunity.com/profiles/76561197960265729/badges/2"), Some(2));
        assert_eq!(community_badge_id("https://steamcommunity.com/id/x/badges/13/"), Some(13));
        assert_eq!(community_badge_id("https://steamcommunity.com/profiles/1/gamecards/413150/"), None);
    }
}
