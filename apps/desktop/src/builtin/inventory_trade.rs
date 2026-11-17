//! A first, minimal trade offer window: pick who to trade with (a friend,
//! or anyone's trade URL), pick items from both inventories of the same
//! game, add a message, send. Steam then asks you to confirm the offer in
//! the Steam Mobile app, the same as on its website.

use super::{Inventory, hex};
use client::Command;
use egui::{Align, Align2, CornerRadius, Layout, Rect, Sense, Stroke, Ui, pos2, vec2};
use extensions::Ctx;
use i18n::t;
use model::{AppId, InventoryItem, SteamId, TradeAsset, art};
use std::collections::BTreeSet;
use ui::{Theme, icons, widgets as w};

pub struct TradeDialog {
    app: AppId,
    context: u64,
    /// Who the offer goes to: (SteamID, name, trade URL token).
    partner: Option<(SteamId, String, Option<String>)>,
    /// Your items in the offer, and theirs (asset ids).
    give: BTreeSet<u64>,
    take: BTreeSet<u64>,
    search: String,
    url: String,
    url_error: bool,
    message: String,
    sending: bool,
    asked: BTreeSet<SteamId>,
}

impl TradeDialog {
    /// Offer one of your items; the partner is chosen in the window.
    pub fn give(item: &InventoryItem, app: AppId) -> Self {
        Self::new(app, item.context_id, None, [item.asset_id].into(), BTreeSet::new())
    }

    /// Ask someone for one of their items.
    pub fn take(item: &InventoryItem, app: AppId, owner: SteamId, name: String) -> Self {
        Self::new(app, item.context_id, Some((owner, name, None)), BTreeSet::new(), [item.asset_id].into())
    }

    fn new(app: AppId, context: u64, partner: Option<(SteamId, String, Option<String>)>, give: BTreeSet<u64>, take: BTreeSet<u64>) -> Self {
        Self { app, context, partner, give, take, search: String::new(), url: String::new(), url_error: false, message: String::new(), sending: false, asked: BTreeSet::new() }
    }
}

impl Inventory {
    pub(super) fn trade_dialog(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
        let Some(d) = &mut self.trade else { return };
        let mut close = false;
        let me = ctx.state.steam_id;
        // Both inventories of this game, loaded once.
        let load = |who: SteamId, d: &mut TradeDialog| {
            if !ctx.state.inventories.contains_key(&(who, d.app, d.context)) && d.asked.insert(who) {
                ctx.send(Command::LoadInventory { steam_id: who, app_id: d.app, context_id: d.context, language: i18n::language().steam_name().to_string() });
            }
        };
        load(me, d);
        if let Some((p, _, _)) = d.partner.clone() {
            load(p, d);
        }
        let width = (ui.ctx().content_rect().width() - 80.0).clamp(360.0, 760.0);
        w::modal(ui.ctx(), egui::Id::unique("trade-offer")).show(ui.ctx(), |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.label(w::text_bold(t!("New trade offer"), 18.0, t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if w::icon_button(ui, t, icons::X, t!("Close"), false, 0).clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(6.0);
            let Some((partner, name, token)) = d.partner.clone() else {
                pick_partner(ui, t, ctx, d);
                return;
            };
            ui.horizontal(|ui| {
                ui.label(w::text(t!("Trading with {name}", name = name.clone()), 13.5, t.text_dim));
                if w::button(ui, t, None, t!("Change"), w::Kind::Ghost, w::Size::Sm).clicked() {
                    d.partner = None;
                    d.take.clear();
                }
            });
            ui.add_space(8.0);
            let half = (width - 16.0) / 2.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                ui.vertical(|ui| {
                    ui.set_width(half);
                    w::caption(ui, t, &t!("You give ({n})", n = d.give.len()));
                    item_grid(ui, t, ctx, (me, d.app, d.context), &mut d.give, half, "give");
                });
                ui.vertical(|ui| {
                    ui.set_width(half);
                    w::caption(ui, t, &t!("You ask for ({n})", n = d.take.len()));
                    item_grid(ui, t, ctx, (partner, d.app, d.context), &mut d.take, half, "take");
                });
            });
            ui.add_space(10.0);
            ui.add(egui::TextEdit::singleline(&mut d.message).hint_text(t!("Message (optional)")).desired_width(f32::INFINITY).char_limit(128).margin(vec2(10.0, 8.0)));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let ready = !(d.give.is_empty() && d.take.is_empty()) && !d.sending;
                let label = if d.sending { t!("Sending…") } else { t!("Send trade offer") };
                if ui.add_enabled_ui(ready, |ui| w::button(ui, t, Some(icons::SWAP), label, w::Kind::Primary, w::Size::Md)).inner.clicked() {
                    let asset = |id: &u64| TradeAsset { app_id: d.app, context_id: d.context, asset_id: *id, amount: 1 };
                    ctx.send(Command::SendTradeOffer {
                        partner,
                        token: token.clone(),
                        message: d.message.trim().to_string(),
                        give: d.give.iter().map(asset).collect(),
                        take: d.take.iter().map(asset).collect(),
                    });
                    d.sending = true;
                }
                ui.label(w::text(t!("You'll confirm it in the Steam Mobile app."), 12.0, t.text_faint));
            });
        });
        if close || (!d.sending && ui.input(|i| i.key_pressed(egui::Key::Escape))) {
            self.trade = None;
        }
    }

    /// A trade offer was sent (or refused): say so and close the window.
    pub(super) fn trade_sent(&mut self, ctx: &mut Ctx, result: &Result<model::TradeOfferSent, String>) {
        match result {
            Ok(r) if r.needs_confirmation => ctx.toast(t!("Trade offer sent. Confirm it in the Steam Mobile app.")),
            Ok(_) => ctx.toast(t!("Trade offer sent")),
            Err(e) => ctx.toast(t!("Couldn't send the trade offer: {error}", error = i18n::tr_dyn(e))),
        }
        match (result, &mut self.trade) {
            (Ok(_), _) => self.trade = None,
            (Err(_), Some(d)) => d.sending = false,
            _ => {}
        }
    }
}

/// Friends to trade with, or a trade URL for anyone else.
fn pick_partner(ui: &mut Ui, t: &Theme, ctx: &mut Ctx, d: &mut TradeDialog) {
    ui.label(w::text(t!("Who do you want to trade with?"), 13.5, t.text_dim));
    ui.add_space(6.0);
    w::search(ui, t, &mut d.search, t!("Search friends"), ui.available_width());
    ui.add_space(6.0);
    let q = d.search.to_lowercase();
    let friends: Vec<_> = ctx.state.friend_list().into_iter().filter(|f| q.is_empty() || f.name.to_lowercase().contains(&q)).collect();
    egui::ScrollArea::vertical().id_salt("trade-friends").max_height(300.0).show(ui, |ui| {
        if friends.is_empty() {
            ui.label(w::text(t!("No friends match."), 13.0, t.text_faint));
        }
        for f in &friends {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r, t.corner_small(), t.surface_hover);
            }
            let av = Rect::from_min_size(r.min + vec2(6.0, 6.0), vec2(28.0, 28.0));
            w::image_or_skeleton(ui, t, av, &art::avatar(f.avatar_hash.as_deref()), CornerRadius::same(4));
            ui.painter().text(pos2(av.right() + 10.0, r.center().y), Align2::LEFT_CENTER, &f.name, w::font_medium(13.5), t.text);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                d.partner = Some((f.steam_id, f.name.clone(), None));
            }
        }
    });
    ui.add_space(10.0);
    ui.label(w::text(t!("Not friends? Paste their trade URL:"), 12.5, t.text_dim));
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut d.url).hint_text("https://steamcommunity.com/tradeoffer/new/?partner=…&token=…").desired_width(ui.available_width() - 90.0).margin(vec2(10.0, 7.0)));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if w::button(ui, t, None, t!("Use"), w::Kind::Secondary, w::Size::Sm).clicked() || enter {
            match model::parse_trade_url(&d.url) {
                Some((id, token)) => {
                    let name = ctx.state.personas.get(&id).map(|p| p.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_string());
                    d.partner = Some((id, name, token));
                    d.url_error = false;
                }
                None => d.url_error = true,
            }
        }
    });
    if d.url_error {
        ui.label(w::text(t!("That isn't a trade URL. Find yours on Steam under Inventory › Trade Offers › Who can send me Trade Offers."), 12.0, t.danger));
    }
}

/// An inventory's tradable items as small tiles; clicking one puts it in
/// the offer or takes it out.
fn item_grid(ui: &mut Ui, t: &Theme, ctx: &Ctx, key: (SteamId, AppId, u64), picked: &mut BTreeSet<u64>, width: f32, salt: &str) {
    let Some(inv) = ctx.state.inventories.get(&key) else {
        w::loading(ui, t, t!("Loading items…"));
        return;
    };
    if let Some(e) = &inv.error {
        let text = if e == "private" { t!("This inventory is private.").to_string() } else { i18n::tr_dyn(e) };
        ui.label(w::text(text, 12.5, t.danger));
        return;
    }
    let items: Vec<&InventoryItem> = inv.items.iter().filter(|i| i.tradable).collect();
    if items.is_empty() {
        ui.label(w::text(t!("No tradable items in this game."), 12.5, t.text_faint));
        return;
    }
    let tile = 58.0;
    egui::ScrollArea::vertical().id_salt(("trade-grid", salt)).max_height(280.0).show(ui, |ui| {
        ui.set_width(width);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            // Picked ones first, so the offer is easy to read.
            let mut sorted = items.clone();
            sorted.sort_by_key(|i| !picked.contains(&i.asset_id));
            for item in sorted {
                let on = picked.contains(&item.asset_id);
                let (r, resp) = ui.allocate_exact_size(vec2(tile, tile), Sense::click());
                let bg = hex(item.background.as_deref()).map(|c| w::mix(t.raised, c, 0.3)).unwrap_or(t.raised);
                ui.painter().rect_filled(r, t.corner_small(), bg);
                if !w::paint_contain(ui, r.shrink(4.0), &art::econ(&item.icon, 96), Align2::CENTER_CENTER) {
                    w::skeleton(ui, t, r.shrink(4.0), t.corner_small());
                }
                let edge = if on { t.accent } else if resp.hovered() { t.text_dim } else { super::rarity_color(item).unwrap_or(t.border) };
                ui.painter().rect_stroke(r, t.corner_small(), Stroke::new(if on { 2.0 } else { 1.0 }, edge), egui::StrokeKind::Inside);
                if on {
                    ui.painter().text(r.right_top() + vec2(-4.0, 3.0), Align2::RIGHT_TOP, icons::CHECK, w::font_bold(13.0), t.accent);
                }
                if resp.on_hover_text(&item.name).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    if on {
                        picked.remove(&item.asset_id);
                    } else {
                        picked.insert(item.asset_id);
                    }
                }
            }
        });
    });
}
