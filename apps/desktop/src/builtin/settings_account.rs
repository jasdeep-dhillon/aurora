//! Account pages inside Settings, read natively from Steam: account
//! details, purchase history, licenses and your Steam Family. Sensitive
//! account changes (password, email, phone, Steam Guard) stay on Steam's
//! site; Steam Family management runs natively through `FamilyGroups`.

use super::settings::{info_row, web};
use client::Command;
use egui::{Color32, Margin, Rect, Sense, Ui, vec2};
use extensions::Ctx;
use i18n::t;
use model::{FamilyActionDone, FamilyRole, Relationship, SteamId, art};
use ui::{Theme, icons, widgets as w};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum AccountView {
    #[default]
    Home,
    Details,
    Purchases,
    Licenses,
}

#[derive(Clone)]
enum FamilyConfirm {
    Leave { family_id: u64, family_name: String },
    RemoveMember { family_id: u64, steam_id: SteamId, name: String },
    CancelInvite { family_id: u64, steam_id: SteamId, name: String },
    DeclineInvite { family_id: u64, inviter_name: String },
    DeleteFamily { family_id: u64, family_name: String },
}

pub struct AccountPages {
    pub view: AccountView,
    license_filter: String,
    shared_filter: String,
    /// Loads already asked for this visit: security, purchases, licenses, family.
    asked: [bool; 4],
    loading_more: bool,
    family_create_name: String,
    family_renaming: bool,
    family_rename_text: String,
    family_invite_open: bool,
    family_invite_query: String,
    family_invite_role: FamilyRole,
    family_confirm: Option<FamilyConfirm>,
    family_busy: bool,
    family_seen_seq: u64,
    family_notice: Option<(bool, String)>,
}

impl Default for AccountPages {
    fn default() -> Self {
        Self {
            view: AccountView::Home,
            license_filter: String::new(),
            shared_filter: String::new(),
            asked: [false; 4],
            loading_more: false,
            family_create_name: String::new(),
            family_renaming: false,
            family_rename_text: String::new(),
            family_invite_open: false,
            family_invite_query: String::new(),
            family_invite_role: FamilyRole::Adult,
            family_confirm: None,
            family_busy: false,
            family_seen_seq: 0,
            family_notice: None,
        }
    }
}

fn language() -> String {
    i18n::language().steam_name().to_string()
}

fn date(unix: Option<u64>) -> String {
    unix.map(ui::date).unwrap_or_else(|| "—".into())
}

/// "Loading…", or the error with a Retry button. True when Retry is pressed.
fn not_ready<T>(ui: &mut Ui, t: &Theme, r: Option<&Result<T, String>>) -> bool {
    match r {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(w::text(t!("Loading…"), 13.5, t.text_dim));
            });
            false
        }
        Some(Err(e)) => {
            ui.label(w::text(t!("Steam didn't answer: {error}", error = e), 13.5, t.danger));
            ui.add_space(6.0);
            w::button(ui, t, Some(icons::REFRESH), t!("Retry"), w::Kind::Secondary, w::Size::Sm).clicked()
        }
        Some(Ok(_)) => false,
    }
}

fn section(ui: &mut Ui, t: &Theme, title: &str) {
    ui.add_space(14.0);
    ui.label(w::caption_text(t, title));
    ui.add_space(6.0);
}

/// Cells of one table row, left to right, with fixed widths (the last
/// takes what's left). Returns the row's rect.
fn row(ui: &mut Ui, t: &Theme, widths: &[f32], striped: bool, mut cell: impl FnMut(usize, &mut Ui)) -> Rect {
    let frame = egui::Frame::new().fill(if striped { t.surface } else { Color32::TRANSPARENT }).inner_margin(Margin::symmetric(10, 7)).corner_radius(t.corner_small());
    frame
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for (i, wdt) in widths.iter().enumerate() {
                    let width = if i + 1 == widths.len() { ui.available_width().max(*wdt) } else { *wdt };
                    ui.allocate_ui_with_layout(vec2(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_width(width);
                        cell(i, ui);
                    });
                }
            });
        })
        .response
        .rect
}

fn wrapped(ui: &mut Ui, text: impl Into<egui::WidgetText>) {
    ui.add(egui::Label::new(text).wrap());
}

impl AccountPages {
    /// Back to the account overview, above each page.
    fn header(&mut self, ui: &mut Ui, t: &Theme, title: &str, subtitle: Option<&str>) {
        if w::button(ui, t, Some(icons::CARET_LEFT), t!("Account"), w::Kind::Ghost, w::Size::Sm).clicked() {
            self.view = AccountView::Home;
        }
        ui.add_space(4.0);
        w::page_title(ui, t, title, subtitle);
    }

    /// Opening a page asks Steam again (the data can change any time).
    pub fn open(&mut self, view: AccountView) {
        self.view = view;
        self.asked = [false; 4];
    }

    pub fn show(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        match self.view {
            AccountView::Home => {}
            AccountView::Details => self.details(ui, t, ctx),
            AccountView::Purchases => self.purchases(ui, t, ctx),
            AccountView::Licenses => self.licenses(ui, t, ctx),
        }
    }

    pub fn ask_security(&mut self, ctx: &mut Ctx) {
        if !self.asked[0] {
            self.asked[0] = true;
            ctx.send(Command::LoadAccountSecurity);
        }
    }

    fn details(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        self.header(ui, t, t!("Account details"), Some(t!("Your account's contact, security and sign-in details.")));
        self.ask_security(ctx);
        let security = ctx.state.account_security.clone();

        section(ui, t, t!("Contact"));
        let email = ctx.state.email.as_ref().map(|(e, ok)| if *ok { format!("{e}  ·  {}", t!("verified")) } else { format!("{e}  ·  {}", t!("not verified")) }).unwrap_or_else(|| "—".into());
        if info_row(ui, t, t!("Email address"), &email, Some(t!("Change email address"))) {
            web("https://help.steampowered.com/wizard/HelpChangeEmail");
        }
        let phone = match &security {
            Some(Ok(s)) if s.phone_verified => t!("A verified phone number is on your account").to_string(),
            Some(Ok(_)) => t!("No phone number").to_string(),
            _ => "…".into(),
        };
        if info_row(ui, t, t!("Phone number"), &phone, Some(t!("Manage phone number"))) {
            web("https://store.steampowered.com/phone/manage");
        }
        // The store country follows the wallet's currency, like Steam's page.
        let country = model::store_country(ctx.state.wallet.map(|(_, c)| c), &ctx.state.country).map(|c| c.to_uppercase()).unwrap_or_else(|| "—".into());
        info_row(ui, t, t!("Store country"), &country, None);

        section(ui, t, t!("Security"));
        if not_ready(ui, t, security.as_ref()) {
            self.asked[0] = false;
        }
        if let Some(Ok(s)) = &security {
            let guard = if s.authenticator {
                t!("Steam Guard Mobile Authenticator, since {date}", date = date(s.authenticator_since)).to_string()
            } else if s.steam_guard {
                t!("Codes by email, since {date}", date = date(s.steam_guard_since)).to_string()
            } else {
                t!("Off: your account isn't protected").to_string()
            };
            if info_row(ui, t, t!("Steam Guard"), &guard, Some(t!("Manage Steam Guard"))) {
                web("https://store.steampowered.com/twofactor/manage");
            }
            let password = match (s.password_changed, s.password_reset) {
                (Some(c), _) => t!("Last changed: {date}", date = date(Some(c))).to_string(),
                (None, Some(r)) => t!("Last reset: {date}", date = date(Some(r))).to_string(),
                _ => "—".to_string(),
            };
            if info_row(ui, t, t!("Password"), &password, Some(t!("Change password"))) {
                web("https://help.steampowered.com/wizard/HelpChangePassword");
            }
            if s.email_changed.is_some() {
                info_row(ui, t, t!("Email address last changed"), &date(s.email_changed), None);
            }
            if !s.devices.is_empty() {
                section(ui, t, t!("Computers Steam Guard remembers"));
                for d in &s.devices {
                    let name = if d.name.is_empty() { t!("Unnamed computer").to_string() } else { d.name.clone() };
                    let value = [date(d.since), d.ip.clone()].into_iter().filter(|s| !s.is_empty() && s != "—").collect::<Vec<_>>().join("  ·  ");
                    info_row(ui, t, &name, &value, None);
                }
            }
        }

        section(ui, t, t!("Steam account"));
        info_row(ui, t, t!("Account name"), &ctx.state.account, None);
        info_row(ui, t, t!("Steam ID"), &ctx.state.steam_id.to_string(), None);
        let balance = ctx.state.wallet.map(|(b, c)| model::format_money(b, c)).unwrap_or_else(|| "—".into());
        if info_row(ui, t, t!("Steam Wallet"), &balance, Some(t!("Purchase history"))) {
            self.open(AccountView::Purchases);
        }
        let vac = match ctx.state.profiles.get(&ctx.state.steam_id) {
            Some(p) if p.vac_banned => t!("VAC bans on record").to_string(),
            Some(_) => t!("No VAC bans").to_string(),
            None => "…".into(),
        };
        info_row(ui, t, t!("VAC status"), &vac, None);
    }

    fn purchases(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        self.header(ui, t, t!("Purchase history"), Some(t!("Store and in-game purchases, gifts, refunds, wallet and Market transactions.")));
        if !self.asked[1] {
            self.asked[1] = true;
            self.loading_more = false;
            ctx.send(Command::LoadPurchases { language: language(), cursor: None });
        }
        let page = ctx.state.purchases.clone();
        if not_ready(ui, t, page.as_ref()) {
            self.asked[1] = false;
        }
        let Some(Ok(page)) = page else { return };
        if page.rows.is_empty() {
            ui.label(w::text(t!("No purchases yet."), 14.0, t.text_dim));
            return;
        }
        const WIDTHS: [f32; 5] = [92.0, 260.0, 150.0, 96.0, 110.0];
        row(ui, t, &WIDTHS, false, |i, ui| {
            let h = [t!("DATE"), t!("ITEMS"), t!("TYPE"), t!("TOTAL"), t!("WALLET")][i];
            ui.label(w::text_bold(h, 10.5, t.text_faint));
        });
        for (n, p) in page.rows.iter().enumerate() {
            let dim = if p.refunded { t.text_faint } else { t.text };
            row(ui, t, &WIDTHS, n % 2 == 0, |i, ui| match i {
                0 => wrapped(ui, w::text(&p.date, 12.5, t.text_dim)),
                1 => {
                    let mut lines = p.items.iter();
                    if let Some(first) = lines.next() {
                        wrapped(ui, w::text_medium(first, 13.0, dim));
                    }
                    for l in lines {
                        wrapped(ui, w::text(l, 12.0, t.text_dim));
                    }
                }
                2 => {
                    wrapped(ui, w::text(&p.kind, 12.5, dim));
                    if !p.payment.is_empty() {
                        wrapped(ui, w::text(&p.payment, 11.5, t.text_faint));
                    }
                }
                3 => {
                    let mut total = w::text_medium(&p.total, 13.0, dim);
                    if p.refunded {
                        total = total.strikethrough();
                    }
                    wrapped(ui, total);
                }
                _ => {
                    if !p.wallet_change.is_empty() {
                        let up = p.wallet_change.starts_with('+');
                        wrapped(ui, w::text_medium(&p.wallet_change, 12.5, if up { t.play } else { t.text_dim }));
                    }
                    if !p.wallet_balance.is_empty() {
                        wrapped(ui, w::text(&p.wallet_balance, 11.5, t.text_faint));
                    }
                }
            });
        }
        ui.add_space(10.0);
        if let Some(cursor) = &page.more {
            if self.loading_more {
                ui.spinner();
            } else if w::button(ui, t, None, t!("Load more transactions"), w::Kind::Secondary, w::Size::Md).clicked() {
                self.loading_more = true;
                ctx.send(Command::LoadPurchases { language: language(), cursor: Some(cursor.clone()) });
            }
        }
        // A finished "load more" changes the cursor.
        if self.loading_more && ctx.state.purchases.as_ref().and_then(|r| r.as_ref().ok()).is_some_and(|p| p.more != page.more || p.rows.len() != page.rows.len()) {
            self.loading_more = false;
        }
        ui.add_space(6.0);
        ui.label(w::text(t!("For help with a purchase, Steam Support has a page per transaction."), 12.0, t.text_faint));
        if w::button(ui, t, Some(icons::QUESTION), t!("Steam Support"), w::Kind::Ghost, w::Size::Sm).clicked() {
            ctx.navigate("support", None);
        }
    }

    fn licenses(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        self.header(ui, t, t!("Licenses and product key activations"), Some(t!("Every game, DLC and package on your account, and how you got it.")));
        if !self.asked[2] {
            self.asked[2] = true;
            ctx.send(Command::LoadLicenses { language: language() });
        }
        let list = ctx.state.licenses.clone();
        if not_ready(ui, t, list.as_ref()) {
            self.asked[2] = false;
        }
        let Some(Ok(list)) = list else { return };
        ui.horizontal(|ui| {
            w::search(ui, t, &mut self.license_filter, t!("Search licenses"), 320.0);
            ui.add_space(12.0);
            ui.label(w::text(t!("{n} licenses", n = list.len()), 13.0, t.text_dim));
        });
        ui.add_space(8.0);
        let needle = self.license_filter.to_lowercase();
        const WIDTHS: [f32; 3] = [100.0, 440.0, 160.0];
        row(ui, t, &WIDTHS, false, |i, ui| {
            ui.label(w::text_bold([t!("DATE"), t!("ITEM"), t!("ACQUISITION METHOD")][i], 10.5, t.text_faint));
        });
        for (n, l) in list.iter().filter(|l| needle.is_empty() || l.name.to_lowercase().contains(&needle)).enumerate() {
            row(ui, t, &WIDTHS, n % 2 == 0, |i, ui| match i {
                0 => wrapped(ui, w::text(&l.date, 12.5, t.text_dim)),
                1 => wrapped(ui, w::text(&l.name, 13.0, t.text)),
                _ => wrapped(ui, w::text(&l.method, 12.5, t.text_dim)),
            });
        }
    }

    fn sync_family_action(&mut self, ctx: &Ctx) {
        let Some((seq, result)) = &ctx.state.family_action else { return };
        if *seq <= self.family_seen_seq {
            return;
        }
        self.family_seen_seq = *seq;
        self.family_busy = false;
        self.family_notice = Some(match result {
            Ok(FamilyActionDone::Created) => {
                self.family_create_name.clear();
                (false, t!("Steam Family created.").to_string())
            }
            Ok(FamilyActionDone::Renamed) => {
                self.family_renaming = false;
                (false, t!("Family name updated.").to_string())
            }
            Ok(FamilyActionDone::Invited { needs_2fa: true }) => (false, t!("Invitation sent. Confirm it in your Steam Mobile App if prompted.").to_string()),
            Ok(FamilyActionDone::Invited { needs_2fa: false }) => (false, t!("Invitation sent.").to_string()),
            Ok(FamilyActionDone::InviteCancelled) => (false, t!("Invitation cancelled.").to_string()),
            Ok(FamilyActionDone::InviteResent) => (false, t!("Invitation resent.").to_string()),
            Ok(FamilyActionDone::Joined { needs_2fa: true }) => (false, t!("Join request sent. Confirm it in your Steam Mobile App if prompted.").to_string()),
            Ok(FamilyActionDone::Joined { needs_2fa: false }) => (false, t!("You joined the Steam Family.").to_string()),
            Ok(FamilyActionDone::Removed) => (false, t!("Member removed from the family.").to_string()),
            Ok(FamilyActionDone::Left) => (false, t!("You left the Steam Family.").to_string()),
            Ok(FamilyActionDone::Deleted) => (false, t!("Steam Family deleted.").to_string()),
            Err(e) => (true, t!("Steam didn't accept that: {error}", error = e).to_string()),
        });
    }

    fn family_notice_banner(&mut self, ui: &mut Ui, t: &Theme) {
        let Some((is_err, msg)) = self.family_notice.clone() else { return };
        let accent = if is_err { t.danger } else { t.accent };
        egui::Frame::new()
            .fill(accent.gamma_multiply(0.14))
            .stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.45)))
            .corner_radius(t.corner_small())
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    wrapped(ui, w::text(&msg, 13.0, if is_err { t.danger } else { t.text }));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if w::button(ui, t, Some(icons::X), "", w::Kind::Ghost, w::Size::Sm).clicked() {
                            self.family_notice = None;
                        }
                    });
                });
            });
        ui.add_space(10.0);
    }

    fn family_confirm_modal(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let Some(confirm) = self.family_confirm.clone() else { return };
        let (title, body, action_label) = match &confirm {
            FamilyConfirm::Leave { family_name, .. } => (
                t!("Leave {name}?", name = family_name).to_string(),
                t!("You won't be able to join or create another Steam Family for 1 year after joining your previous family.").to_string(),
                t!("Leave family").to_string(),
            ),
            FamilyConfirm::RemoveMember { name, .. } => (
                t!("Remove {name}?", name = name).to_string(),
                t!("They will lose access to shared games, and their slot will be on cooldown for 1 year.").to_string(),
                t!("Remove").to_string(),
            ),
            FamilyConfirm::CancelInvite { name, .. } => (
                t!("Cancel invitation?").to_string(),
                t!("Cancel the pending family invitation for {name}?", name = name).to_string(),
                t!("Cancel invite").to_string(),
            ),
            FamilyConfirm::DeclineInvite { inviter_name, .. } => (
                t!("Decline invitation?").to_string(),
                t!("Decline the Steam Family invitation from {name}?", name = inviter_name).to_string(),
                t!("Decline").to_string(),
            ),
            FamilyConfirm::DeleteFamily { family_name, .. } => (
                t!("Delete {name}?", name = family_name).to_string(),
                t!("This permanently deletes the Steam Family.").to_string(),
                t!("Delete family").to_string(),
            ),
        };
        let (mut confirmed, mut cancelled) = (false, false);
        w::modal(ui.ctx(), egui::Id::unique("confirm-family-action")).show(ui.ctx(), |ui| {
            ui.set_width(460.0);
            ui.label(w::text_bold(&title, 18.0, t.text));
            ui.add_space(8.0);
            wrapped(ui, w::text(&body, 13.5, t.text_dim));
            ui.add_space(20.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                confirmed = w::button(ui, t, None, &action_label, w::Kind::Danger, w::Size::Md).clicked();
                cancelled = w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Md).clicked();
            });
        });
        if confirmed {
            self.family_confirm = None;
            self.family_busy = true;
            let me = ctx.state.steam_id;
            match confirm {
                FamilyConfirm::Leave { family_id, .. } => ctx.send(Command::RemoveFromFamily { family_id, steam_id: me, language: language() }),
                FamilyConfirm::RemoveMember { family_id, steam_id, .. } => ctx.send(Command::RemoveFromFamily { family_id, steam_id, language: language() }),
                FamilyConfirm::CancelInvite { family_id, steam_id, .. } => ctx.send(Command::CancelFamilyInvite { family_id, steam_id, language: language() }),
                FamilyConfirm::DeclineInvite { family_id, .. } => ctx.send(Command::CancelFamilyInvite { family_id, steam_id: me, language: language() }),
                FamilyConfirm::DeleteFamily { family_id, .. } => ctx.send(Command::DeleteFamily { family_id, language: language() }),
            }
        } else if cancelled {
            self.family_confirm = None;
        }
    }

    fn family_invite_modal(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, family: &model::Family) {
        if !self.family_invite_open {
            return;
        }
        let mut close = false;
        let mut chosen: Option<SteamId> = None;
        let me = ctx.state.steam_id;
        let already = |id: SteamId| id == me || family.members.iter().any(|m| m.steam_id == id) || family.invites.iter().any(|(i, _)| *i == id);

        w::modal(ui.ctx(), egui::Id::unique("family-invite-member")).show(ui.ctx(), |ui| {
            ui.set_width(480.0);
            ui.horizontal(|ui| {
                ui.label(w::text_bold(t!("Invite to family"), 18.0, t.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if w::button(ui, t, Some(icons::X), "", w::Kind::Ghost, w::Size::Sm).clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(w::text(t!("Role:"), 13.0, t.text_dim));
                let adult_kind = if self.family_invite_role == FamilyRole::Adult { w::Kind::Primary } else { w::Kind::Secondary };
                let child_kind = if self.family_invite_role == FamilyRole::Child { w::Kind::Primary } else { w::Kind::Secondary };
                if w::button(ui, t, None, t!("Adult"), adult_kind, w::Size::Sm).clicked() {
                    self.family_invite_role = FamilyRole::Adult;
                }
                if w::button(ui, t, None, t!("Child"), child_kind, w::Size::Sm).clicked() {
                    self.family_invite_role = FamilyRole::Child;
                }
            });
            ui.add_space(4.0);
            let role_note = if self.family_invite_role == FamilyRole::Child {
                t!("Children can play shared games with parental controls set by adults.")
            } else {
                t!("Adults can manage invites, remove members and set parental controls.")
            };
            wrapped(ui, w::text(role_note, 12.0, t.text_dim));
            ui.add_space(10.0);
            w::search(ui, t, &mut self.family_invite_query, t!("Search friends or enter a SteamID64"), ui.available_width());
            ui.add_space(8.0);

            let trimmed = self.family_invite_query.trim();
            if let Ok(parsed_id) = trimmed.parse::<u64>()
                && parsed_id >= 76_561_197_960_265_728
                && !already(parsed_id)
            {
                ui.horizontal(|ui| {
                    ui.label(w::text(t!("SteamID: {id}", id = parsed_id), 13.0, t.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if w::button(ui, t, Some(icons::USER_PLUS), t!("Invite"), w::Kind::Primary, w::Size::Sm).clicked() {
                            chosen = Some(parsed_id);
                        }
                    });
                });
                ui.add_space(6.0);
            }

            let needle = trimmed.to_lowercase();
            let mut friends: Vec<(SteamId, String, Option<String>)> = ctx
                .state
                .friends
                .iter()
                .filter(|(id, rel)| **rel == Relationship::Friend && !already(**id))
                .map(|(id, _)| {
                    let p = ctx.state.persona(*id);
                    let name = if p.display_name().is_empty() { id.to_string() } else { p.display_name() };
                    (*id, name, p.avatar_hash.clone())
                })
                .filter(|(id, name, _)| needle.is_empty() || name.to_lowercase().contains(&needle) || id.to_string().contains(&needle))
                .collect();
            friends.sort_by_key(|(_, name, _)| name.to_lowercase());

            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                if friends.is_empty() {
                    ui.label(w::text(t!("No eligible friends found. You can also paste a 64-bit SteamID above."), 12.5, t.text_dim));
                } else {
                    for (id, name, avatar_hash) in friends {
                        let p = ctx.state.persona(id);
                        egui::Frame::new().inner_margin(Margin::symmetric(6, 4)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                w::avatar(ui, t, &art::avatar(avatar_hash.as_deref()), 28.0, Some(crate::shell::status_color(t, &p)));
                                ui.label(w::text_medium(&name, 13.5, t.text));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if w::button(ui, t, Some(icons::USER_PLUS), t!("Invite"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                        chosen = Some(id);
                                    }
                                });
                            });
                        });
                    }
                }
            });
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if w::button(ui, t, None, t!("Close"), w::Kind::Ghost, w::Size::Sm).clicked() {
                    close = true;
                }
            });
        });

        if let Some(steam_id) = chosen {
            self.family_invite_open = false;
            self.family_invite_query.clear();
            self.family_busy = true;
            ctx.send(Command::InviteToFamily {
                family_id: family.id,
                steam_id,
                role: self.family_invite_role,
                language: language(),
            });
        } else if close {
            self.family_invite_open = false;
        }
    }

    pub fn family(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        self.sync_family_action(ctx);
        w::page_title(ui, t, t!("Family"), Some(t!("Steam Families: share your library and set limits for children.")));
        self.family_notice_banner(ui, t);
        if !self.asked[3] {
            self.asked[3] = true;
            ctx.send(Command::LoadFamily { language: language() });
        }
        let status = ctx.state.family.clone();
        if not_ready(ui, t, status.as_ref()) {
            self.asked[3] = false;
        }
        let Some(Ok(status)) = status else { return };
        let me = ctx.state.steam_id;

        if self.family_busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(w::text(t!("Updating Steam Family…"), 13.0, t.text_dim));
            });
            ui.add_space(8.0);
        }

        let Some(family) = &status.family else {
            if !status.invites.is_empty() {
                section(ui, t, t!("Pending invitations"));
                for inv in &status.invites {
                    let p = ctx.state.persona(inv.inviter);
                    let inviter_name = if p.display_name().is_empty() { inv.inviter.to_string() } else { p.display_name() };
                    w::panel(t).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            w::avatar(ui, t, &art::avatar(p.avatar_hash.as_deref()), 36.0, Some(crate::shell::status_color(t, &p)));
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(w::text_semibold(t!("{name} invited you to their family.", name = &inviter_name), 14.0, t.text));
                                    let (label, color) = match inv.role {
                                        FamilyRole::Adult => (t!("Adult"), t.accent),
                                        FamilyRole::Child => (t!("Child"), t.away),
                                        FamilyRole::Unknown => (t!("Member"), t.text_dim),
                                    };
                                    w::pill(ui, label, color, color.gamma_multiply(0.15));
                                });
                                if inv.awaiting_2fa {
                                    ui.label(w::text(t!("Awaiting Steam Guard confirmation"), 12.0, t.away));
                                }
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.add_enabled_ui(!self.family_busy, |ui| {
                                    if w::button(ui, t, Some(icons::CHECK), t!("Accept"), w::Kind::Primary, w::Size::Sm).clicked() {
                                        self.family_busy = true;
                                        ctx.send(Command::JoinFamily { family_id: inv.family_id, language: language() });
                                    }
                                    if w::button(ui, t, Some(icons::X), t!("Decline"), w::Kind::Ghost, w::Size::Sm).clicked() {
                                        self.family_confirm = Some(FamilyConfirm::DeclineInvite {
                                            family_id: inv.family_id,
                                            inviter_name: inviter_name.clone(),
                                        });
                                    }
                                });
                            });
                        });
                    });
                    ui.add_space(6.0);
                }
                ui.add_space(8.0);
            }

            w::panel(t).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(w::text_semibold(t!("You're not in a Steam Family"), 16.0, t.text));
                ui.add_space(4.0);
                wrapped(ui, w::text(t!("A Steam Family shares games between up to six accounts, with parental controls for children."), 13.5, t.text_dim));
                if status.cooldown > 0 {
                    ui.add_space(6.0);
                    ui.label(w::text(t!("You can join a family again in {days} days.", days = status.cooldown.div_ceil(86_400)), 13.0, t.away));
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!self.family_busy && status.cooldown == 0, |ui| {
                        w::search(ui, t, &mut self.family_create_name, t!("New family name"), 260.0);
                        let can_create = !self.family_create_name.trim().is_empty();
                        ui.add_enabled_ui(can_create, |ui| {
                            if w::button(ui, t, Some(icons::USERS_THREE), t!("Create family"), w::Kind::Primary, w::Size::Md).clicked() {
                                self.family_busy = true;
                                ctx.send(Command::CreateFamily { name: self.family_create_name.trim().to_string(), language: language() });
                            }
                        });
                    });
                });
                ui.add_space(8.0);
                if w::button(ui, t, Some(icons::GEAR), t!("Open on Steam"), w::Kind::Ghost, w::Size::Sm)
                    .on_hover_text(t!("Parental controls and purchase requests open on Steam's site"))
                    .clicked()
                {
                    web("https://store.steampowered.com/account/familymanagement");
                }
            });
            self.family_confirm_modal(ui, t, ctx);
            return;
        };

        let is_adult = status.role == FamilyRole::Adult;
        ui.horizontal(|ui| {
            if self.family_renaming {
                w::search(ui, t, &mut self.family_rename_text, t!("Family name"), 240.0);
                let can_save = !self.family_rename_text.trim().is_empty() && !self.family_busy;
                ui.add_enabled_ui(can_save, |ui| {
                    if w::button(ui, t, Some(icons::CHECK), t!("Save"), w::Kind::Primary, w::Size::Sm).clicked() {
                        self.family_busy = true;
                        ctx.send(Command::RenameFamily {
                            family_id: family.id,
                            name: self.family_rename_text.trim().to_string(),
                            language: language(),
                        });
                    }
                });
                if w::button(ui, t, None, t!("Cancel"), w::Kind::Ghost, w::Size::Sm).clicked() {
                    self.family_renaming = false;
                }
            } else {
                ui.label(w::text_bold(&family.name, 22.0, t.text));
                if is_adult && w::button(ui, t, None, t!("Rename"), w::Kind::Ghost, w::Size::Sm).clicked() {
                    self.family_rename_text = family.name.clone();
                    self.family_renaming = true;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if w::button(ui, t, Some(icons::GEAR), t!("Open on Steam"), w::Kind::Ghost, w::Size::Sm)
                    .on_hover_text(t!("Parental controls and purchase requests open on Steam's site"))
                    .clicked()
                {
                    web("https://store.steampowered.com/account/familymanagement");
                }
                ui.add_enabled_ui(!self.family_busy, |ui| {
                    if is_adult && family.members.len() <= 1 {
                        if w::button(ui, t, Some(icons::TRASH), t!("Delete family"), w::Kind::Danger, w::Size::Sm).clicked() {
                            self.family_confirm = Some(FamilyConfirm::DeleteFamily {
                                family_id: family.id,
                                family_name: family.name.clone(),
                            });
                        }
                    } else if w::button(ui, t, Some(icons::SIGN_OUT), t!("Leave family"), w::Kind::Secondary, w::Size::Sm).clicked() {
                        self.family_confirm = Some(FamilyConfirm::Leave {
                            family_id: family.id,
                            family_name: family.name.clone(),
                        });
                    }
                    if is_adult && family.free_spots > 0 && w::button(ui, t, Some(icons::USER_PLUS), t!("Invite member"), w::Kind::Primary, w::Size::Sm).clicked() {
                        self.family_invite_query.clear();
                        self.family_invite_role = FamilyRole::Adult;
                        self.family_invite_open = true;
                    }
                });
            });
        });
        let mut facts = vec![t!("{n} members", n = family.members.len()).to_string(), t!("{n} free spots", n = family.free_spots).to_string()];
        if !family.country.is_empty() {
            facts.push(family.country.clone());
        }
        ui.label(w::text(facts.join("  ·  "), 13.0, t.text_dim));
        if family.slot_cooldown > 0 {
            ui.label(w::text(t!("A new member can join in {days} days.", days = family.slot_cooldown.div_ceil(86_400)), 12.5, t.away));
        }

        section(ui, t, t!("Members"));
        for m in &family.members {
            let p = ctx.state.persona(m.steam_id);
            let name = if p.display_name().is_empty() { m.steam_id.to_string() } else { p.display_name() };
            w::panel(t).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    w::avatar(ui, t, &art::avatar(p.avatar_hash.as_deref()), 40.0, Some(crate::shell::status_color(t, &p)));
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(w::text_semibold(&name, 14.5, t.text));
                            if m.steam_id == me {
                                ui.label(w::text(t!("(you)"), 12.5, t.text_faint));
                            }
                            let (label, color) = match m.role {
                                FamilyRole::Adult => (t!("Adult"), t.accent),
                                FamilyRole::Child => (t!("Child"), t.away),
                                FamilyRole::Unknown => (t!("Member"), t.text_dim),
                            };
                            w::pill(ui, label, color, color.gamma_multiply(0.15));
                        });
                        let mut line = t!("Joined {date}", date = date(Some(m.joined).filter(|j| *j > 0))).to_string();
                        if m.cooldown > 0 {
                            line.push_str(&format!("  ·  {}", t!("can join another family in {days} days", days = m.cooldown.div_ceil(86_400))));
                        }
                        ui.label(w::text(line, 12.5, t.text_dim));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if is_adult && m.steam_id != me {
                            ui.add_enabled_ui(!self.family_busy, |ui| {
                                if w::button(ui, t, Some(icons::TRASH), t!("Remove"), w::Kind::Ghost, w::Size::Sm).clicked() {
                                    self.family_confirm = Some(FamilyConfirm::RemoveMember {
                                        family_id: family.id,
                                        steam_id: m.steam_id,
                                        name: name.clone(),
                                    });
                                }
                            });
                        }
                        if w::button(ui, t, None, t!("Profile"), w::Kind::Ghost, w::Size::Sm).clicked() {
                            ctx.navigate("profile", Some(m.steam_id.to_string()));
                        }
                    });
                });
            });
            ui.add_space(6.0);
        }
        if !family.invites.is_empty() {
            section(ui, t, t!("Invited"));
            for (id, role) in &family.invites {
                let p = ctx.state.persona(*id);
                let name = if p.display_name().is_empty() { id.to_string() } else { p.display_name() };
                w::panel(t).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        w::avatar(ui, t, &art::avatar(p.avatar_hash.as_deref()), 32.0, Some(crate::shell::status_color(t, &p)));
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(w::text_semibold(&name, 14.0, t.text));
                                let (label, color) = match role {
                                    FamilyRole::Adult => (t!("Adult"), t.accent),
                                    FamilyRole::Child => (t!("Child"), t.away),
                                    FamilyRole::Unknown => (t!("Member"), t.text_dim),
                                };
                                w::pill(ui, label, color, color.gamma_multiply(0.15));
                            });
                            ui.label(w::text(t!("Hasn't accepted yet"), 12.0, t.text_dim));
                        });
                        if is_adult {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                ui.add_enabled_ui(!self.family_busy, |ui| {
                                    if w::button(ui, t, Some(icons::X), t!("Cancel invite"), w::Kind::Ghost, w::Size::Sm).clicked() {
                                        self.family_confirm = Some(FamilyConfirm::CancelInvite {
                                            family_id: family.id,
                                            steam_id: *id,
                                            name: name.clone(),
                                        });
                                    }
                                    if w::button(ui, t, Some(icons::REFRESH), t!("Resend"), w::Kind::Secondary, w::Size::Sm).clicked() {
                                        self.family_busy = true;
                                        ctx.send(Command::ResendFamilyInvite { family_id: family.id, steam_id: *id });
                                    }
                                });
                            });
                        }
                    });
                });
                ui.add_space(6.0);
            }
        }

        section(ui, t, t!("Family library"));
        let shareable = family.shared.iter().filter(|a| !a.excluded).count();
        ui.horizontal(|ui| {
            w::search(ui, t, &mut self.shared_filter, t!("Search the family library"), 320.0);
            ui.add_space(12.0);
            ui.label(w::text(t!("{n} games can be shared", n = shareable), 13.0, t.text_dim));
        });
        ui.add_space(8.0);
        let needle = self.shared_filter.to_lowercase();
        for a in family.shared.iter().filter(|a| needle.is_empty() || a.name.to_lowercase().contains(&needle)) {
            let resp = egui::Frame::new().inner_margin(Margin::symmetric(6, 4)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(92.0, 43.0), Sense::hover());
                    w::image_or_skeleton(ui, t, r, &art::header(a.app_id), t.corner_small());
                    ui.vertical(|ui| {
                        ui.add_space(4.0);
                        ui.label(w::text_medium(&a.name, 13.5, if a.excluded { t.text_faint } else { t.text }));
                        let owners: Vec<String> = a.owners.iter().map(|o| if *o == me { t!("you").to_string() } else { ctx.state.persona(*o).display_name() }).collect();
                        let mut line = t!("Owned by {names}", names = owners.join(", ")).to_string();
                        if a.excluded {
                            line.push_str(&format!("  ·  {}", t!("can't be shared")));
                        }
                        ui.label(w::text(line, 12.0, t.text_dim));
                    });
                });
            });
            if resp.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                ctx.navigate("store", Some(a.app_id.to_string()));
            }
        }

        self.family_invite_modal(ui, t, ctx, family);
        self.family_confirm_modal(ui, t, ctx);
    }
}
