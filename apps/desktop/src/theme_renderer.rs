//! Generic `egui` renderer for declarative [`themes::Node`] UI trees.
//!
//! Renders any theme's `layout.shell` and `layout.library` trees using the
//! CSS-like box model (`NodeStyle`) and Aurora's built-in interactive widgets.

use crate::app::SteamApp;
use client::Command;
use egui::{Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Rect, Sense, Stroke, Ui, pos2, vec2};
use extensions::{Action, Ctx, Placement};
use i18n::t;
use model::{PersonaState, art};
use themes::{
    AlignMode, BackgroundFit, BorderEdge, Direction, FontWeight, GradientSpec, IndicatorStyle, Justify,
    MenuItemSpec, Node, NodeStyle, PanelSide, ResolvedTheme, SizeVal,
};
use ui::{Theme, icons, widgets as w};

/// Paint a custom background texture inside `rect` with the configured fit,
/// opacity, and optional tint overlay.
pub fn paint_background_texture(
    ui: &Ui,
    rect: Rect,
    tex: &egui::TextureHandle,
    fit: BackgroundFit,
    opacity: f32,
    tint: Option<Color32>,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.001 && tint.is_none() {
        return;
    }
    let painter = ui.painter_at(rect);
    let img_tint = Color32::from_white_alpha((opacity * 255.0).round() as u8);
    let tex_size = tex.size_vec2();
    if opacity > 0.001 && tex_size.x > 0.0 && tex_size.y > 0.0 && rect.width() > 0.0 && rect.height() > 0.0 {
        match fit {
            BackgroundFit::Cover => {
                let rect_aspect = rect.width() / rect.height();
                let tex_aspect = tex_size.x / tex_size.y;
                let uv = if tex_aspect > rect_aspect {
                    let w = rect_aspect / tex_aspect;
                    let pad = (1.0 - w) * 0.5;
                    Rect::from_min_max(pos2(pad, 0.0), pos2(1.0 - pad, 1.0))
                } else {
                    let h = tex_aspect / rect_aspect;
                    let pad = (1.0 - h) * 0.5;
                    Rect::from_min_max(pos2(0.0, pad), pos2(1.0, 1.0 - pad))
                };
                painter.image(tex.id(), rect, uv, img_tint);
            }
            BackgroundFit::Contain => {
                let scale = (rect.width() / tex_size.x).min(rect.height() / tex_size.y);
                let dst = Rect::from_center_size(rect.center(), tex_size * scale);
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                painter.image(tex.id(), dst, uv, img_tint);
            }
            BackgroundFit::Tile => {
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                let step_x = tex_size.x.max(16.0);
                let step_y = tex_size.y.max(16.0);
                let mut y = rect.top();
                while y < rect.bottom() {
                    let mut x = rect.left();
                    while x < rect.right() {
                        let tile = Rect::from_min_size(pos2(x, y), vec2(step_x, step_y));
                        painter.image(tex.id(), tile, uv, img_tint);
                        x += step_x;
                    }
                    y += step_y;
                }
            }
        }
    }
    if let Some(tint) = tint {
        painter.rect_filled(rect, CornerRadius::ZERO, tint);
    }
}

/// Paint a linear gradient fill across `rect`.
pub fn paint_gradient(painter: &egui::Painter, rect: Rect, grad: &GradientSpec, t: &Theme) {
    let from = grad.from.resolve(t);
    let to = grad.to.resolve(t);
    let mut mesh = egui::Mesh::default();
    let (c0, c1, c2, c3) = match grad.direction {
        Direction::Vertical => (from, from, to, to),
        Direction::Horizontal => (from, to, to, from),
    };
    let uv = egui::epaint::WHITE_UV;
    mesh.colored_vertex(rect.left_top(), c0);
    mesh.colored_vertex(rect.right_top(), c1);
    mesh.colored_vertex(rect.right_bottom(), c2);
    mesh.colored_vertex(rect.left_bottom(), c3);
    for v in &mut mesh.vertices {
        v.uv = uv;
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// Paint per-edge borders (`border_top`, `border_bottom`, `border_left`, `border_right`).
pub fn paint_edge_borders(painter: &egui::Painter, rect: Rect, style: &NodeStyle, t: &Theme) {
    let draw = |edge: Option<BorderEdge>, p1: egui::Pos2, p2: egui::Pos2| {
        if let Some(e) = edge {
            if e.width > 0.0 {
                painter.line_segment([p1, p2], Stroke::new(e.width, e.color.resolve(t)));
            }
        }
    };
    draw(
        style.border_top,
        pos2(rect.left(), rect.top() + style.border_top.map(|e| e.width * 0.5).unwrap_or(0.0)),
        pos2(rect.right(), rect.top() + style.border_top.map(|e| e.width * 0.5).unwrap_or(0.0)),
    );
    draw(
        style.border_bottom,
        pos2(rect.left(), rect.bottom() - style.border_bottom.map(|e| e.width * 0.5).unwrap_or(0.0)),
        pos2(rect.right(), rect.bottom() - style.border_bottom.map(|e| e.width * 0.5).unwrap_or(0.0)),
    );
    draw(
        style.border_left,
        pos2(rect.left() + style.border_left.map(|e| e.width * 0.5).unwrap_or(0.0), rect.top()),
        pos2(rect.left() + style.border_left.map(|e| e.width * 0.5).unwrap_or(0.0), rect.bottom()),
    );
    draw(
        style.border_right,
        pos2(rect.right() - style.border_right.map(|e| e.width * 0.5).unwrap_or(0.0), rect.top()),
        pos2(rect.right() - style.border_right.map(|e| e.width * 0.5).unwrap_or(0.0), rect.bottom()),
    );
}

pub fn font_for(weight: Option<FontWeight>, size: f32) -> FontId {
    match weight.unwrap_or(FontWeight::Regular) {
        FontWeight::Regular => w::font(size),
        FontWeight::Medium => w::font_medium(size),
        FontWeight::Semibold => w::font_semibold(size),
        FontWeight::Bold => w::font_bold(size),
    }
}

fn align_to_egui(a: AlignMode) -> Align {
    match a {
        AlignMode::Start => Align::Min,
        AlignMode::Center => Align::Center,
        AlignMode::End => Align::Max,
    }
}

fn frame_from_style(style: &NodeStyle, t: &Theme, default_fill: Color32) -> egui::Frame {
    let fill = style.bg.map(|c| c.resolve(t)).unwrap_or(default_fill);
    let stroke = style
        .border
        .filter(|b| b.width > 0.0)
        .map(|b| Stroke::new(b.width, b.color.resolve(t)))
        .unwrap_or(Stroke::NONE);
    let radius = style.radius.map(CornerRadius::same).unwrap_or(CornerRadius::ZERO);
    let inner_margin = style.padding.map(|e| e.to_egui_margin()).unwrap_or(Margin::ZERO);
    let outer_margin = style.margin.map(|e| e.to_egui_margin()).unwrap_or(Margin::ZERO);
    egui::Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(radius)
        .inner_margin(inner_margin)
        .outer_margin(outer_margin)
}

fn icon_for_name(name: &str) -> &str {
    match name {
        "gear" | "settings" => icons::GEAR,
        "download" | "downloads" => icons::DOWNLOAD,
        "info" | "support" => icons::INFO,
        "key" => icons::KEY,
        "tag" => icons::TAG,
        "plugs" | "plugins" => icons::PLUGS,
        "refresh" => icons::REFRESH,
        "users" | "friends" => icons::USERS,
        "users_three" | "community" => icons::USERS_THREE,
        "power" | "exit" => icons::POWER,
        "books" | "library" => icons::BOOKS,
        "store" => icons::STORE,
        "chats" => icons::CHATS,
        "plus" => icons::PLUS,
        "house" | "home" => icons::HOUSE,
        "grid" => icons::GRID,
        "search" => icons::SEARCH,
        "play" => icons::PLAY,
        "bell" => icons::BELL,
        "sliders" => icons::SLIDERS,
        other => other,
    }
}

// ── Shell Renderer ──────────────────────────────────────────────────────────

struct ShellRenderCtx<'a, 'b, 'c> {
    app: &'a mut SteamApp,
    t: Theme,
    actions: &'a mut Vec<Action>,
    resolved: &'a ResolvedTheme,
    bg_tex: Option<&'a egui::TextureHandle>,
    logo_tex: Option<&'a egui::TextureHandle>,
    paint_bg: Option<&'b mut (dyn FnMut(&egui::Painter, Rect) + 'b)>,
    paint_chrome: Option<&'c mut (dyn FnMut(&egui::Painter, Rect) + 'c)>,
    panel_seq: usize,
}

#[allow(clippy::too_many_arguments)]
pub fn render_shell(
    app: &mut SteamApp,
    ui: &mut Ui,
    actions: &mut Vec<Action>,
    resolved: &ResolvedTheme,
    bg_tex: Option<&egui::TextureHandle>,
    logo_tex: Option<&egui::TextureHandle>,
    paint_bg: Option<&mut dyn FnMut(&egui::Painter, Rect)>,
    paint_chrome: Option<&mut dyn FnMut(&egui::Painter, Rect)>,
) {
    let t = app.prefs.theme.clone();
    let mut rctx = ShellRenderCtx {
        app,
        t,
        actions,
        resolved,
        bg_tex,
        logo_tex,
        paint_bg,
        paint_chrome,
        panel_seq: 0,
    };
    for node in &resolved.layout.shell {
        render_shell_node(&mut rctx, ui, node);
    }
}

fn dispatch_shell_action(app: &mut SteamApp, ui: &Ui, actions: &mut Vec<Action>, action: &str) {
    if let Some(rest) = action.strip_prefix("navigate:") {
        if let Some((page, arg)) = rest.split_once(':') {
            actions.push(Action::Navigate { page: page.into(), arg: Some(arg.into()) });
        } else {
            actions.push(Action::Navigate { page: rest.into(), arg: None });
        }
    } else if let Some(url) = action.strip_prefix("open_url:") {
        crate::links::open_http(url);
    } else {
        match action {
            "add_game" => actions.push(Action::Navigate { page: "library".into(), arg: Some("add".into()) }),
            "activate_product" => app.activate = Some(Default::default()),
            "open_friends" => app.friends_window.open = !app.friends_window.open,
            "toggle_friends_drawer" => app.prefs.friends_open = !app.prefs.friends_open,
            "back" => app.go_back(),
            "forward" => app.go_forward(),
            "check_updates" => crate::updater::check(ui.ctx().clone(), true),
            "logout" => app.client.send(Command::LogOut),
            "exit" => app.request_exit(ui.ctx()),
            _ => {}
        }
    }
}

fn interpolate_shell_text(app: &SteamApp, template: &str) -> String {
    if !template.contains('{') {
        return template.to_string();
    }
    let username = if app.state.persona_name.is_empty() {
        app.state.account.as_str()
    } else {
        app.state.persona_name.as_str()
    };
    let wallet = app
        .state
        .wallet
        .map(|(b, c)| model::format_money(b, c))
        .unwrap_or_default();
    let installed_count = app.local.games.len().to_string();
    let total_games = app
        .state
        .apps
        .as_ref()
        .map(|a| a.values().filter(|o| !o.is_dlc()).count())
        .unwrap_or(app.local.games.len())
        .to_string();
    let online_friends = app
        .state
        .friend_list()
        .iter()
        .filter(|p| p.state != PersonaState::Offline)
        .count()
        .to_string();
    let playing_game = app
        .state
        .playing
        .and_then(|id| app.state.app_name(id))
        .unwrap_or("")
        .to_string();
    let downloads_label = app
        .state
        .downloads
        .values()
        .find(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed))
        .and_then(|d| app.state.app_name(d.app_id).map(String::from))
        .unwrap_or_else(|| t!("Downloads").to_string());

    template
        .replace("{username}", username)
        .replace("{wallet}", &wallet)
        .replace("{installed_count}", &installed_count)
        .replace("{total_games}", &total_games)
        .replace("{online_friends}", &online_friends)
        .replace("{playing_game}", &playing_game)
        .replace("{downloads_label}", &downloads_label)
}

fn render_shell_node(rctx: &mut ShellRenderCtx<'_, '_, '_>, ui: &mut Ui, node: &Node) {
    let t = rctx.t.clone();
    match node {
        Node::Panel { side, size, resizable, drag_window, style, children } => {
            rctx.panel_seq += 1;
            let pid = format!("theme-panel-{}", rctx.panel_seq);
            let default_fill = match side {
                PanelSide::Center => t.bg,
                _ => t.chrome,
            };
            let fill = style.bg.map(|c| c.resolve(&t)).unwrap_or(default_fill);
            let padding = style.padding.map(|e| e.to_egui_margin()).unwrap_or(Margin::ZERO);
            let frame = egui::Frame::new().fill(fill).inner_margin(Margin::ZERO);

            let render_body = |rctx: &mut ShellRenderCtx<'_, '_, '_>, ui: &mut Ui, is_center: bool| {
                let full_rect = ui.max_rect();
                if let Some(grad) = &style.bg_gradient {
                    paint_gradient(ui.painter(), full_rect, grad, &t);
                }
                if is_center {
                    if let Some(tex) = rctx.bg_tex {
                        paint_background_texture(
                            ui,
                            full_rect,
                            tex,
                            rctx.resolved.background_fit,
                            rctx.resolved.background_opacity,
                            rctx.resolved.background_tint,
                        );
                    }
                    if let Some(f) = rctx.paint_bg.as_deref_mut() {
                        f(ui.painter(), full_rect);
                    }
                } else if matches!(side, PanelSide::Top | PanelSide::Left | PanelSide::Bottom) {
                    if let Some(f) = rctx.paint_chrome.as_deref_mut() {
                        f(ui.painter(), full_rect);
                    }
                }
                if *drag_window {
                    crate::chrome::drag_area(ui, full_rect, true);
                }
                egui::Frame::new().inner_margin(padding).show(ui, |ui| {
                    if let Some(gap) = style.gap {
                        ui.spacing_mut().item_spacing = vec2(gap, gap);
                    } else {
                        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    }
                    for child in children {
                        render_shell_node(rctx, ui, child);
                    }
                });
                paint_edge_borders(ui.painter(), full_rect, style, &t);
            };

            match side {
                PanelSide::Top => {
                    let mut p = egui::Panel::top(pid).resizable(*resizable).frame(frame);
                    if let Some(sz) = size {
                        p = if *resizable { p.default_size(*sz) } else { p.exact_size(*sz) };
                    }
                    p.show(ui, |ui| render_body(rctx, ui, false));
                }
                PanelSide::Bottom => {
                    let mut p = egui::Panel::bottom(pid).resizable(*resizable).frame(frame);
                    if let Some(sz) = size {
                        p = if *resizable { p.default_size(*sz) } else { p.exact_size(*sz) };
                    }
                    p.show(ui, |ui| render_body(rctx, ui, false));
                }
                PanelSide::Left => {
                    let mut p = egui::Panel::left(pid).resizable(*resizable).frame(frame);
                    if let Some(sz) = size {
                        p = if *resizable { p.default_size(*sz) } else { p.exact_size(*sz) };
                    }
                    p.show(ui, |ui| render_body(rctx, ui, false));
                }
                PanelSide::Right => {
                    let mut p = egui::Panel::right(pid).resizable(*resizable).frame(frame);
                    if let Some(sz) = size {
                        p = if *resizable { p.default_size(*sz) } else { p.exact_size(*sz) };
                    }
                    p.show(ui, |ui| render_body(rctx, ui, false));
                }
                PanelSide::Center => {
                    egui::CentralPanel::default().frame(frame).show(ui, |ui| render_body(rctx, ui, true));
                }
            }
        }

        Node::Row { justify, align, wrap, style, children } => {
            render_row_generic(
                ui,
                &t,
                *justify,
                *align,
                *wrap,
                style,
                children,
                &mut |ui, child| render_shell_node(rctx, ui, child),
            );
        }

        Node::Col { align, style, children } => {
            render_col_generic(ui, &t, *align, style, children, &mut |ui, child| {
                render_shell_node(rctx, ui, child);
            });
        }

        Node::Box { style, children } => {
            render_box_generic(ui, &t, style, children, &mut |ui, child| {
                render_shell_node(rctx, ui, child);
            });
        }

        Node::Scroll { direction, style, children } => {
            render_scroll_generic(ui, &t, *direction, style, children, &mut |ui, child| {
                render_shell_node(rctx, ui, child);
            });
        }

        Node::Spacer { size, flex, .. } => {
            if !flex {
                ui.add_space(size.unwrap_or(8.0));
            }
        }

        Node::Divider { orientation, color, thickness, .. } => {
            let c = color.map(|cr| cr.resolve(&t)).unwrap_or(t.border);
            match orientation {
                Direction::Horizontal => {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), (*thickness).max(1.0)), Sense::hover());
                    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(*thickness, c));
                }
                Direction::Vertical => {
                    let (r, _) = ui.allocate_exact_size(vec2((*thickness).max(1.0), ui.available_height().min(28.0)), Sense::hover());
                    ui.painter().vline(r.center().x, r.y_range(), Stroke::new(*thickness, c));
                }
            }
        }

        Node::Logo { size, .. } => {
            let sz = (*size).max(12.0);
            let (rect, _) = ui.allocate_exact_size(vec2(sz, sz), Sense::hover());
            if let Some(tex) = rctx.logo_tex {
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
            } else {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                w::logo(&mut child, &t, sz);
            }
        }

        Node::Text { value, style } => {
            let mut text = interpolate_shell_text(rctx.app, value);
            if style.uppercase.unwrap_or(false) {
                text = text.to_uppercase();
            }
            let color = style.color.map(|c| c.resolve(&t)).unwrap_or(t.text);
            let font = font_for(style.font_weight, style.font_size.unwrap_or(13.0));
            ui.label(egui::RichText::new(text).font(font).color(color));
        }

        Node::Button { label, icon, action, style } => {
            let resp = render_styled_button(ui, &t, label, icon.as_deref(), style);
            if resp.clicked() {
                dispatch_shell_action(rctx.app, ui, rctx.actions, action);
            }
        }

        Node::Menu { label, icon, items, style } => {
            let resp = render_styled_button(ui, &t, label, icon.as_deref(), style);
            egui::Popup::menu(&resp).width(240.0).show(|ui| {
                for item in items {
                    match item {
                        MenuItemSpec::Separator => {
                            ui.separator();
                        }
                        MenuItemSpec::Item { label, icon, action } => {
                            let ic = icon.as_deref().map(icon_for_name).unwrap_or("");
                            if w::menu_item(ui, &t, ic, &i18n::tr_dyn(label)).clicked() {
                                dispatch_shell_action(rctx.app, ui, rctx.actions, action);
                                ui.close();
                            }
                        }
                    }
                }
            });
        }

        Node::NavTabs {
            direction,
            show_icons,
            uppercase,
            profile_as_username,
            indicator,
            indicator_height,
            tab_gap,
            style,
        } => {
            render_nav_tabs(
                rctx,
                ui,
                *direction,
                *show_icons,
                *uppercase,
                *profile_as_username,
                *indicator,
                *indicator_height,
                *tab_gap,
                style,
            );
        }

        Node::BackForward { size, style } => {
            let sz = (*size).max(18.0);
            let color = style.color.map(|c| c.resolve(&t)).unwrap_or(t.text_dim);
            let hover_c = style.hover_color.map(|c| c.resolve(&t)).unwrap_or(t.text);
            let font_sz = style.font_size.unwrap_or((sz * 0.68).clamp(13.0, 18.0));
            let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
            let draw_arrow = |ui: &mut Ui, icon: &str, tip: &str, enabled: bool| -> bool {
                ui.add_enabled_ui(enabled, |ui| {
                    let (rect, resp) = ui.allocate_exact_size(vec2(sz, sz), Sense::click());
                    let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.08);
                    let fg = if !enabled { color.gamma_multiply(0.35) } else { w::mix(color, hover_c, h) };
                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, w::font(font_sz), fg);
                    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                })
                .inner
            };
            if rtl {
                if draw_arrow(ui, icons::ARROW_RIGHT, t!("Forward"), !rctx.app.forward.is_empty()) {
                    rctx.app.go_forward();
                }
                if draw_arrow(ui, icons::ARROW_LEFT, t!("Back"), !rctx.app.back.is_empty()) {
                    rctx.app.go_back();
                }
            } else {
                if draw_arrow(ui, icons::ARROW_LEFT, t!("Back"), !rctx.app.back.is_empty()) {
                    rctx.app.go_back();
                }
                if draw_arrow(ui, icons::ARROW_RIGHT, t!("Forward"), !rctx.app.forward.is_empty()) {
                    rctx.app.go_forward();
                }
            }
        }

        Node::WindowControls { variant, height, .. } => {
            crate::chrome::window_buttons_variant(ui, &t, *height, true, *variant);
        }

        Node::Account { show_avatar, show_name, show_wallet, show_caret, avatar_size, style } => {
            render_account_node(rctx, ui, *show_avatar, *show_name, *show_wallet, *show_caret, *avatar_size, style);
        }

        Node::Wallet { .. } => {
            if let Some((balance, currency)) = rctx.app.state.wallet.filter(|(b, _)| *b > 0 || rctx.app.state.wallet_pending > 0) {
                crate::shell::wallet_button(ui, &t, balance, currency, rctx.app.state.wallet_pending, rctx.actions);
            }
        }

        Node::Notifications { size, highlight_unread_bg, style } => {
            let Some(mut data) = rctx.app.state.notifications.clone() else { return };
            data.items.sort_by_key(|n| std::cmp::Reverse(n.time));
            let waiting = data.pending_gifts + data.pending_friends + data.confirmations;
            let badge = (data.unread + waiting) as usize;
            let sz = (*size).max(20.0);
            let w_px = match style.width {
                Some(SizeVal::Px(w)) => w.max(sz),
                _ => (sz * 1.55).max(32.0),
            };
            let (rect, resp) = ui.allocate_exact_size(vec2(w_px, sz), Sense::click());
            let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
            let radius = style.radius.map(CornerRadius::same).unwrap_or_else(|| t.corner_small());
            let base_bg = style.bg.map(|c| c.resolve(&t)).unwrap_or(t.raised);
            let bg = if badge > 0 {
                let hi = highlight_unread_bg.map(|c| c.resolve(&t)).unwrap_or(t.play);
                w::mix(hi, w::mix(hi, Color32::WHITE, 0.14), hover)
            } else {
                w::mix(base_bg, t.surface_hover, hover)
            };
            let fg = if badge > 0 {
                Color32::WHITE
            } else {
                w::mix(style.color.map(|c| c.resolve(&t)).unwrap_or(t.text_dim), t.text, hover)
            };
            ui.painter().rect_filled(rect, radius, bg);
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, icons::BELL, w::font((sz * 0.56).clamp(11.0, 16.0)), fg);
            let r = resp.on_hover_text(t!("Notifications")).on_hover_cursor(egui::CursorIcon::PointingHand);
            crate::shell::notifications_popup(rctx.app, &r, &t, &data, rctx.actions);
        }

        Node::FriendsButton { size, style } => {
            let sz = (*size).max(20.0);
            let w_px = match style.width {
                Some(SizeVal::Px(w)) => w.max(sz),
                _ => (sz * 1.55).max(32.0),
            };
            let unread = rctx.app.state.total_unread();
            let (rect, resp) = ui.allocate_exact_size(vec2(w_px, sz), Sense::click());
            let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
            let active = rctx.app.prefs.friends_open;
            let radius = style.radius.map(CornerRadius::same).unwrap_or_else(|| t.corner_small());
            let base_bg = style.bg.map(|c| c.resolve(&t)).unwrap_or(t.raised);
            let bg = if active {
                w::active_fill(&t)
            } else {
                w::mix(base_bg, t.surface_hover, hover)
            };
            let fg = if active {
                t.accent
            } else {
                w::mix(style.color.map(|c| c.resolve(&t)).unwrap_or(t.text_dim), t.text, hover)
            };
            ui.painter().rect_filled(rect, radius, bg);
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, icons::USERS, w::font((sz * 0.56).clamp(11.0, 16.0)), fg);
            if unread > 0 {
                let c = pos2(rect.right() - 4.0, rect.top() + 4.0);
                ui.painter().circle_filled(c, 4.5, t.accent);
            }
            let fr = resp.on_hover_text(t!("Friends")).on_hover_cursor(egui::CursorIcon::PointingHand);
            if fr.clicked() {
                rctx.app.prefs.friends_open = !rctx.app.prefs.friends_open;
            }
            crate::shell::friends_hover(rctx.app, ui, &t, &fr, rctx.actions);
        }

        Node::DownloadsBar { width, style } => {
            let w_px = width.unwrap_or(340.0);
            let h_px = ui.available_height().clamp(24.0, 32.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(w_px, h_px), Sense::click());
            let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
            let active = rctx.app.state.downloads.values().find(|d| !matches!(d.phase, model::DownloadPhase::Done | model::DownloadPhase::Failed)).cloned();
            let p = ui.painter();
            let fg = w::mix(style.color.map(|c| c.resolve(&t)).unwrap_or(t.text_dim), t.text, hover);
            if let Some(d) = active {
                let frac = (d.disk_done as f64 / d.disk_total.max(1) as f64).clamp(0.0, 1.0) as f32;
                let pct = format!("{}%", (frac * 100.0).round() as u32);
                let name = rctx.app.state.app_name(d.app_id).unwrap_or("Updating").to_string();
                let ic_rect = Rect::from_min_size(pos2(rect.left(), rect.center().y - 10.0), vec2(20.0, 20.0));
                let ic_url = rctx.app.state.apps.as_ref().and_then(|a| a.get(&d.app_id)).and_then(|o| o.icon.as_deref()).map(|h| art::icon(d.app_id, h));
                let hdr = art::header(d.app_id);
                let urls: Vec<&str> = ic_url.as_deref().into_iter().chain([hdr.as_str()]).collect();
                if !w::paint_cover_first(ui, ic_rect, &urls, CornerRadius::same(2)) {
                    p.rect_filled(ic_rect, CornerRadius::same(2), t.raised);
                }
                let tx = ic_rect.right() + 8.0;
                p.text(pos2(tx, rect.top() + 3.0), Align2::LEFT_TOP, name, w::font(12.0), fg);
                p.text(pos2(rect.right(), rect.top() + 3.0), Align2::RIGHT_TOP, pct, w::font(12.0), fg);
                let track = Rect::from_min_max(pos2(tx, rect.bottom() - 5.0), pos2(rect.right(), rect.bottom() - 2.0));
                p.rect_filled(track, CornerRadius::same(1), Color32::from_rgb(0x0B, 0x0E, 0x12));
                let fill = Rect::from_min_size(track.min, vec2(track.width() * frac, track.height()));
                p.rect_filled(fill, CornerRadius::same(1), if d.paused { t.text_faint } else { t.accent });
            } else {
                let label = format!("{}  {}", icons::DOWNLOAD, t!("Manage Downloads"));
                let galley = p.layout_no_wrap(label, w::font(12.0), fg);
                p.galley(pos2(rect.center().x - galley.size().x * 0.5, rect.center().y - galley.size().y * 0.5 - 2.0), galley, fg);
                let track = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 3.5), vec2((w_px * 0.55).min(180.0), 2.5));
                p.rect_filled(track, CornerRadius::same(1), Color32::from_rgb(0x0B, 0x0E, 0x12));
            }
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                rctx.actions.push(Action::Navigate { page: "downloads".into(), arg: None });
            }
        }

        Node::FriendsDrawer { width, style } => {
            if rctx.app.prefs.friends_open && rctx.app.prefs.page != "community" {
                let fill = style.bg.map(|c| c.resolve(&t)).unwrap_or(t.chrome);
                egui::Panel::right("friends")
                    .exact_size((*width).clamp(200.0, 420.0))
                    .resizable(false)
                    .frame(egui::Frame::new().fill(fill).inner_margin(Margin { left: 14, right: 10, top: 16, bottom: 8 }))
                    .show(ui, |ui| crate::shell::friends_drawer(rctx.app, ui, &t, rctx.actions));
            }
        }

        Node::PageContent { .. } => {
            crate::shell::show_active_page(rctx.app, ui, &t, rctx.actions);
        }

        // Library-only nodes inside a shell tree are no-ops.
        Node::LibrarySearch { .. }
        | Node::LibraryFilters { .. }
        | Node::LibraryList { .. }
        | Node::WhatsNewShelf { .. }
        | Node::RecentGamesShelf { .. }
        | Node::AllGamesGrid { .. }
        | Node::LibraryHomeOrDetail { .. } => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn render_nav_tabs(
    rctx: &mut ShellRenderCtx<'_, '_, '_>,
    ui: &mut Ui,
    direction: Direction,
    show_icons: bool,
    uppercase: bool,
    profile_as_username: bool,
    indicator: IndicatorStyle,
    indicator_height: f32,
    tab_gap: f32,
    style: &NodeStyle,
) {
    let t = rctx.t.clone();
    let pages = rctx.app.registry.pages();
    let current = rctx.app.prefs.page.clone();
    let fs = style.font_size.unwrap_or(15.0);
    let font = font_for(style.font_weight.or(Some(FontWeight::Bold)), fs);
    let base_c = style.color.map(|c| c.resolve(&t)).unwrap_or(t.text_dim);
    let hover_c = style.hover_color.map(|c| c.resolve(&t)).unwrap_or(t.text);
    let active_c = style.active_color.map(|c| c.resolve(&t)).unwrap_or(t.accent);
    let radius = style.radius.map(CornerRadius::same).unwrap_or_else(|| t.corner_small());

    let draw_tab = |ui: &mut Ui, icon: Option<&str>, label: &str, selected: bool, full_w: bool| -> egui::Response {
        let display = if uppercase || style.uppercase.unwrap_or(false) {
            label.to_uppercase()
        } else {
            label.to_string()
        };
        let galley = ui.painter().layout_no_wrap(display, font.clone(), Color32::PLACEHOLDER);
        let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
        let pad_x = if indicator == IndicatorStyle::Pill { 12.0 } else { 4.0 };
        let tab_w = if full_w {
            ui.available_width()
        } else {
            galley.size().x + icon_w + pad_x * 2.0
        };
        let tab_h = match style.height {
            Some(SizeVal::Px(h)) => h,
            _ => (galley.size().y + 14.0).max(32.0),
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(tab_w, tab_h), Sense::click());
        let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
        let sel = ui.ctx().animate_bool_with_time(resp.id.with("sel"), selected, 0.14);
        let fg = w::mix(w::mix(base_c, hover_c, hover), active_c, sel);
        let p = ui.painter();

        match indicator {
            IndicatorStyle::None => {}
            IndicatorStyle::Pill => {
                let bg = style.active_bg.map(|c| c.resolve(&t)).unwrap_or_else(|| w::active_fill(&t));
                let hbg = style.hover_bg.map(|c| c.resolve(&t)).unwrap_or(t.surface_hover);
                let fill = w::mix(w::mix(Color32::TRANSPARENT, hbg, hover), bg, sel);
                if fill != Color32::TRANSPARENT {
                    p.rect_filled(rect, radius, fill);
                }
            }
            IndicatorStyle::Underline => {
                if sel > 0.01 {
                    let bar_w = (galley.size().x + icon_w).max(16.0) * sel;
                    let cx = rect.left() + pad_x + (galley.size().x + icon_w) * 0.5;
                    let bar_y = rect.center().y + galley.size().y * 0.5 + 3.0;
                    let bar = Rect::from_min_size(pos2(cx - bar_w * 0.5, bar_y), vec2(bar_w, indicator_height.max(2.0)));
                    p.rect_filled(bar, CornerRadius::same(1), active_c);
                }
            }
            IndicatorStyle::LeftBar => {
                let hbg = style.hover_bg.map(|c| c.resolve(&t)).unwrap_or(t.surface_hover.gamma_multiply(0.5));
                let abg = style.active_bg.map(|c| c.resolve(&t)).unwrap_or_else(|| w::active_fill(&t));
                let fill = w::mix(w::mix(Color32::TRANSPARENT, hbg, hover), abg, sel);
                if fill != Color32::TRANSPARENT {
                    p.rect_filled(rect, radius, fill);
                }
                if sel > 0.01 {
                    let bar_h = (rect.height() - 10.0).max(12.0) * sel;
                    let bar = Rect::from_center_size(pos2(rect.left() + 2.0, rect.center().y), vec2(indicator_height.max(2.5), bar_h));
                    p.rect_filled(bar, CornerRadius::same(1), active_c);
                }
            }
        }

        let mut x = rect.left() + pad_x;
        if let Some(ic) = icon {
            p.text(pos2(x + 7.0, rect.center().y), Align2::CENTER_CENTER, ic, w::font(fs), fg);
            x += icon_w;
        }
        p.galley(pos2(x, rect.center().y - galley.size().y * 0.5), galley, fg);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    };

    let render_all = |rctx: &mut ShellRenderCtx<'_, '_, '_>, ui: &mut Ui, vertical: bool| {
        for p in pages.iter().filter(|p| p.placement == Placement::Main) {
            let icon = (show_icons && !p.icon.is_empty()).then_some(p.icon.as_str());
            let title = if profile_as_username && p.id == "profile" {
                if !rctx.app.state.persona_name.is_empty() {
                    rctx.app.state.persona_name.clone()
                } else if !rctx.app.state.account.is_empty() {
                    rctx.app.state.account.clone()
                } else {
                    i18n::tr_dyn(&p.title)
                }
            } else {
                i18n::tr_dyn(&p.title)
            };
            let tab = draw_tab(ui, icon, &title, current == p.id, vertical);
            if tab.clicked() {
                rctx.actions.push(Action::Navigate { page: p.id.clone(), arg: None });
            }
            if !vertical {
                let items = crate::shell::tab_menu_items(rctx.app, &p.id);
                if let Some((page, arg)) = crate::shell::hover_menu(ui, &t, egui::Id::unique(("tab-menu", &p.id)), &tab, &items) {
                    rctx.actions.push(Action::Navigate { page, arg });
                }
            }
        }
        let plugin_pages: Vec<_> = pages.iter().filter(|p| p.placement == Placement::Apps).collect();
        if !plugin_pages.is_empty() {
            let active = plugin_pages.iter().any(|p| p.id == current);
            let icon = show_icons.then_some(icons::PLUGS);
            let label = format!("{} {}", t!("Plugins"), icons::CARET_DOWN);
            let resp = draw_tab(ui, icon, &label, active, vertical);
            egui::Popup::menu(&resp).width(220.0).show(|ui| {
                for p in &plugin_pages {
                    let label = format!("{}   {}", p.icon, p.title);
                    if ui.add(egui::Button::selectable(current == p.id, w::text_medium(label, 14.0, t.text)).min_size(vec2(200.0, 34.0))).clicked() {
                        rctx.actions.push(Action::Navigate { page: p.id.clone(), arg: None });
                    }
                }
            });
        }
    };

    match direction {
        Direction::Horizontal => {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = tab_gap;
                render_all(rctx, ui, false);
            });
        }
        Direction::Vertical => {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = tab_gap;
                render_all(rctx, ui, true);
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_account_node(
    rctx: &mut ShellRenderCtx<'_, '_, '_>,
    ui: &mut Ui,
    show_avatar: bool,
    show_name: bool,
    show_wallet: bool,
    show_caret: bool,
    avatar_size: f32,
    style: &NodeStyle,
) {
    let t = rctx.t.clone();
    let me = rctx.app.state.me().cloned().unwrap_or_default();
    let name = if rctx.app.state.persona_name.is_empty() {
        &rctx.app.state.account
    } else {
        &rctx.app.state.persona_name
    };
    let short: String = if name.chars().count() > 16 {
        name.chars().take(15).chain(std::iter::once('…')).collect()
    } else {
        name.clone()
    };
    let color = crate::shell::status_color(&t, &me);
    let name_color = if me.in_game() {
        t.in_game
    } else if me.state == PersonaState::Offline {
        t.text
    } else {
        color
    };
    let fs = style.font_size.unwrap_or(if avatar_size <= 20.0 { 12.0 } else { 13.5 });
    let pad = style.padding.unwrap_or(themes::Edges::symmetric(2.0, 6.0));
    let height = match style.height {
        Some(SizeVal::Px(h)) => h,
        _ => (avatar_size + pad.top + pad.bottom).max(24.0),
    };
    let name_g = show_name.then(|| ui.painter().layout_no_wrap(short, w::font_medium(fs), name_color));
    let wallet_g = if show_wallet {
        rctx.app
            .state
            .wallet
            .filter(|(b, _)| *b > 0 || rctx.app.state.wallet_pending > 0)
            .map(|(b, c)| ui.painter().layout_no_wrap(model::format_money(b, c), w::font(fs - 0.5), t.text_dim))
    } else {
        None
    };

    let mut total_w = pad.left + pad.right;
    if show_avatar {
        total_w += avatar_size;
    }
    if let Some(g) = &name_g {
        if show_avatar {
            total_w += 6.0;
        }
        total_w += g.size().x;
    }
    if let Some(g) = &wallet_g {
        total_w += 8.0 + g.size().x;
    }
    if show_caret {
        total_w += 16.0;
    }

    let (rect, resp) = ui.allocate_exact_size(vec2(total_w, height), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    let bg = style.bg.map(|c| c.resolve(&t)).unwrap_or(t.raised);
    let hbg = style.hover_bg.map(|c| c.resolve(&t)).unwrap_or(t.surface_hover);
    let radius = style.radius.map(CornerRadius::same).unwrap_or_else(|| t.corner_small());
    ui.painter().rect_filled(rect, radius, w::mix(bg, hbg, hover));

    let mut x = rect.left() + pad.left;
    if show_avatar {
        let av_rect = Rect::from_min_size(pos2(x, rect.center().y - avatar_size * 0.5), vec2(avatar_size, avatar_size));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(av_rect));
        w::avatar_rounded(&mut child, &t, &art::avatar(me.avatar_hash.as_deref()), avatar_size, None, CornerRadius::ZERO);
        // Steam-style 2px vertical status stripe along the right edge of the avatar.
        ui.painter().rect_filled(Rect::from_min_size(pos2(av_rect.right(), av_rect.top()), vec2(2.0, avatar_size)), CornerRadius::ZERO, color);
        x += avatar_size + 8.0;
    }
    if let Some(g) = name_g {
        let gw = g.size().x;
        ui.painter().galley(pos2(x, rect.center().y - g.size().y * 0.5), g, name_color);
        x += gw + 4.0;
    }
    if show_caret {
        ui.painter().text(pos2(x + 4.0, rect.center().y), Align2::CENTER_CENTER, icons::CARET_DOWN, w::font(10.0), name_color);
        x += 14.0;
    }
    if let Some(g) = wallet_g {
        ui.painter().galley(pos2(x, rect.center().y - g.size().y * 0.5), g, t.text_dim);
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    crate::shell::account_menu_popup(rctx.app, &resp, &t, &me, rctx.actions);
}

fn render_styled_button(ui: &mut Ui, t: &Theme, label: &str, icon: Option<&str>, style: &NodeStyle) -> egui::Response {
    let fs = style.font_size.unwrap_or(12.5);
    let font = font_for(style.font_weight, fs);
    let raw = if label.is_empty() { String::new() } else { i18n::tr_dyn(label) };
    let display = if style.uppercase.unwrap_or(false) { raw.to_uppercase() } else { raw };
    let is_badge_btn = icon == Some("plus_box") || icon == Some("friends_box");
    let ic = if is_badge_btn { None } else { icon.map(icon_for_name) };
    let text_str = match (ic, display.is_empty()) {
        (Some(i), true) => i.to_string(),
        (Some(i), false) => format!("{i}  {display}"),
        (None, _) => display,
    };
    let galley = ui.painter().layout_no_wrap(text_str, font.clone(), Color32::PLACEHOLDER);
    let pad = style.padding.unwrap_or(themes::Edges::symmetric(4.0, 8.0));
    let extra_badge_w = if is_badge_btn { 26.0 } else { 0.0 };
    let w_px = match style.width {
        Some(SizeVal::Px(w)) => w,
        Some(SizeVal::Fill) => ui.available_width().max(galley.size().x + extra_badge_w + pad.left + pad.right),
        _ => galley.size().x + extra_badge_w + pad.left + pad.right,
    };
    let h_px = match style.height {
        Some(SizeVal::Px(h)) => h,
        _ => (galley.size().y + pad.top + pad.bottom).max(24.0),
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(w_px, h_px), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.08);
    let radius = style.radius.map(CornerRadius::same).unwrap_or(CornerRadius::same(2));
    let base_bg = style.bg.map(|c| c.resolve(t)).unwrap_or(Color32::TRANSPARENT);
    let hover_bg = style.hover_bg.map(|c| c.resolve(t)).unwrap_or(t.surface_hover.gamma_multiply(0.7));
    let fill = w::mix(base_bg, hover_bg, hover);
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, radius, fill);
    }
    if let Some(b) = style.border {
        if b.width > 0.0 {
            ui.painter().rect_stroke(rect, radius, Stroke::new(b.width, b.color.resolve(t)), egui::StrokeKind::Inside);
        }
    }
    let base_fg = style.color.map(|c| c.resolve(t)).unwrap_or(t.text_dim);
    let hover_fg = style.hover_color.map(|c| c.resolve(t)).unwrap_or(t.text);
    let fg = w::mix(base_fg, hover_fg, hover);
    if icon == Some("grid") && label.is_empty() {
        // Draw crisp 2x2 squares directly (Steam Collections icon, never misses a font glyph).
        let c = rect.center();
        let s = 6.0;
        let g = 2.0;
        for dx in [-s - g * 0.5, g * 0.5] {
            for dy in [-s - g * 0.5, g * 0.5] {
                ui.painter().rect_filled(Rect::from_min_size(pos2(c.x + dx, c.y + dy), vec2(s, s)), CornerRadius::same(1), fg);
            }
        }
        return resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    if icon == Some("plus_box") || icon == Some("friends_box") {
        let right_side = icon == Some("friends_box");
        let lbl_g = ui.painter().layout_no_wrap(i18n::tr_dyn(label), font, fg);
        let box_sz = 18.0;
        let gap = 7.0;
        let box_bg = w::mix(Color32::from_rgb(0x3B, 0x42, 0x4D), Color32::from_rgb(0x4E, 0x57, 0x66), hover);
        let glyph_c = Color32::from_rgb(0x17, 0x1D, 0x25);
        if right_side {
            let lx = rect.left() + pad.left;
            ui.painter().galley(pos2(lx, rect.center().y - lbl_g.size().y * 0.5), lbl_g, fg);
            let b = Rect::from_min_size(pos2(rect.right() - pad.right - box_sz, rect.center().y - box_sz * 0.5), vec2(box_sz, box_sz));
            ui.painter().rect_filled(b, CornerRadius::same(2), box_bg);
            ui.painter().text(b.center(), Align2::CENTER_CENTER, icons::USERS, w::font(11.5), glyph_c);
        } else {
            let b = Rect::from_min_size(pos2(rect.left() + pad.left, rect.center().y - box_sz * 0.5), vec2(box_sz, box_sz));
            ui.painter().rect_filled(b, CornerRadius::same(2), box_bg);
            ui.painter().text(b.center(), Align2::CENTER_CENTER, icons::PLUS, w::font_bold(12.0), glyph_c);
            ui.painter().galley(pos2(b.right() + gap, rect.center().y - lbl_g.size().y * 0.5), lbl_g, fg);
        }
        return resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    let text_x = match style.align.unwrap_or(AlignMode::Center) {
        AlignMode::Start => rect.left() + pad.left,
        AlignMode::Center => rect.center().x - galley.size().x * 0.5,
        AlignMode::End => rect.right() - pad.right - galley.size().x,
    };
    ui.painter().galley(pos2(text_x, rect.center().y - galley.size().y * 0.5), galley, fg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ── Generic Container Helpers (shared by Shell & Library renderers) ─────────

fn is_flex_spacer(node: &Node) -> bool {
    matches!(node, Node::Spacer { flex: true, .. })
}

#[allow(clippy::too_many_arguments)]
fn render_row_generic(
    ui: &mut Ui,
    t: &Theme,
    justify: Justify,
    align: AlignMode,
    wrap: bool,
    style: &NodeStyle,
    children: &[Node],
    render_child: &mut dyn FnMut(&mut Ui, &Node),
) {
    let gap = style.gap.unwrap_or(6.0);
    let cross = align_to_egui(align);
    let frame = frame_from_style(style, t, Color32::TRANSPARENT);

    let show_inner = |ui: &mut Ui, render_child: &mut dyn FnMut(&mut Ui, &Node)| {
        if let Some(SizeVal::Px(h)) = style.height {
            ui.set_min_height(h);
            ui.set_max_height(h);
        }
        if let Some(SizeVal::Px(w)) = style.width {
            ui.set_width(w);
        } else if matches!(style.width, Some(SizeVal::Fill)) || style.bg.is_some() || style.bg_gradient.is_some() {
            ui.set_width(ui.available_width());
        }

        // Split around flex spacers so left / center / right groups align pixel-perfectly.
        let flex_indices: Vec<usize> = children
            .iter()
            .enumerate()
            .filter_map(|(i, n)| is_flex_spacer(n).then_some(i))
            .collect();

        if wrap {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(gap, gap);
                for c in children {
                    render_child(ui, c);
                }
            });
            return;
        }

        let row_h = match style.height {
            Some(SizeVal::Px(h)) => h,
            _ => ui.available_height().min(48.0).max(24.0),
        };

        if flex_indices.len() == 1 {
            let split = flex_indices[0];
            let left = &children[..split];
            let right = &children[split + 1..];
            ui.allocate_ui_with_layout(vec2(ui.available_width(), row_h), Layout::left_to_right(cross), |ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for c in left {
                    render_child(ui, c);
                }
                ui.with_layout(Layout::right_to_left(cross), |ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for c in right.iter().rev() {
                        render_child(ui, c);
                    }
                });
            });
        } else if flex_indices.len() >= 2 {
            let first = flex_indices[0];
            let last = *flex_indices.last().unwrap();
            let left = &children[..first];
            let center = &children[first + 1..last];
            let right = &children[last + 1..];
            let (row_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
            let mut lr = ui.new_child(egui::UiBuilder::new().max_rect(row_rect).layout(Layout::left_to_right(cross)));
            lr.spacing_mut().item_spacing.x = gap;
            for c in left {
                render_child(&mut lr, c);
            }
            lr.with_layout(Layout::right_to_left(cross), |ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for c in right.iter().rev() {
                    render_child(ui, c);
                }
            });
            // Center items centered horizontally inside `row_rect`.
            let center_w = (row_rect.width() * 0.45).clamp(160.0, 420.0);
            let center_rect = Rect::from_center_size(row_rect.center(), vec2(center_w, row_h));
            let mut mid = ui.new_child(egui::UiBuilder::new().max_rect(center_rect).layout(Layout::centered_and_justified(egui::Direction::LeftToRight)));
            mid.spacing_mut().item_spacing.x = gap;
            for c in center {
                if !is_flex_spacer(c) {
                    render_child(&mut mid, c);
                }
            }
        } else {
            match justify {
                Justify::End => {
                    ui.allocate_ui_with_layout(vec2(ui.available_width(), row_h), Layout::right_to_left(cross), |ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for c in children.iter().rev() {
                            render_child(ui, c);
                        }
                    });
                }
                Justify::SpaceBetween if children.len() >= 2 => {
                    ui.allocate_ui_with_layout(vec2(ui.available_width(), row_h), Layout::left_to_right(cross), |ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        render_child(ui, &children[0]);
                        ui.with_layout(Layout::right_to_left(cross), |ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            for c in children[1..].iter().rev() {
                                render_child(ui, c);
                            }
                        });
                    });
                }
                _ => {
                    ui.allocate_ui_with_layout(vec2(ui.available_width(), row_h), Layout::left_to_right(cross), |ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        // Check if any child wants `SizeVal::Fill` width so we can give it remaining width.
                        let fill_count = children.iter().filter(|c| node_width(c) == Some(SizeVal::Fill)).count();
                        if fill_count == 1 && children.len() >= 2 {
                            let fixed_sum: f32 = children
                                .iter()
                                .filter(|c| node_width(c) != Some(SizeVal::Fill))
                                .map(|c| match node_width(c) {
                                    Some(SizeVal::Px(w)) => w,
                                    _ => 32.0,
                                })
                                .sum();
                            let fill_w = (ui.available_width() - fixed_sum - gap * (children.len() - 1) as f32).max(60.0);
                            for c in children {
                                if node_width(c) == Some(SizeVal::Fill) {
                                    ui.allocate_ui(vec2(fill_w, row_h), |ui| render_child(ui, c));
                                } else {
                                    render_child(ui, c);
                                }
                            }
                        } else {
                            for c in children {
                                render_child(ui, c);
                            }
                        }
                    });
                }
            }
        }
    };

    let resp = frame.show(ui, |ui| {
        if let Some(grad) = &style.bg_gradient {
            paint_gradient(ui.painter(), ui.max_rect(), grad, t);
        }
        show_inner(ui, render_child);
    });
    paint_edge_borders(ui.painter(), resp.response.rect, style, t);
}

fn node_width(node: &Node) -> Option<SizeVal> {
    match node {
        Node::Button { style, .. }
        | Node::Box { style, .. }
        | Node::Row { style, .. }
        | Node::Col { style, .. }
        | Node::Text { style, .. }
        | Node::LibrarySearch { style, .. } => style.width,
        _ => None,
    }
}

fn render_col_generic(
    ui: &mut Ui,
    t: &Theme,
    align: AlignMode,
    style: &NodeStyle,
    children: &[Node],
    render_child: &mut dyn FnMut(&mut Ui, &Node),
) {
    let gap = style.gap.unwrap_or(6.0);
    let frame = frame_from_style(style, t, Color32::TRANSPARENT);
    let resp = frame.show(ui, |ui| {
        if let Some(SizeVal::Px(w)) = style.width {
            ui.set_width(w);
        }
        ui.with_layout(Layout::top_down(align_to_egui(align)), |ui| {
            ui.spacing_mut().item_spacing.y = gap;
            for c in children {
                render_child(ui, c);
            }
        });
    });
    paint_edge_borders(ui.painter(), resp.response.rect, style, t);
}

fn render_box_generic(
    ui: &mut Ui,
    t: &Theme,
    style: &NodeStyle,
    children: &[Node],
    render_child: &mut dyn FnMut(&mut Ui, &Node),
) {
    let gap = style.gap.unwrap_or(8.0);
    let bg_idx = style.bg_gradient.as_ref().map(|_| ui.painter().add(egui::Shape::Noop));
    let frame = frame_from_style(style, t, Color32::TRANSPARENT);
    let resp = frame.show(ui, |ui| {
        match style.width {
            Some(SizeVal::Px(w)) => ui.set_width(w),
            Some(SizeVal::Fill) | None => ui.set_width(ui.available_width()),
            Some(SizeVal::Auto) => {}
        }
        if let Some(SizeVal::Px(h)) = style.height {
            ui.set_min_height(h);
        }
        ui.spacing_mut().item_spacing.y = gap;
        for c in children {
            render_child(ui, c);
        }
    });
    let rect = resp.response.rect;
    if let (Some(grad), Some(idx)) = (&style.bg_gradient, bg_idx) {
        let from = grad.from.resolve(t);
        let to = grad.to.resolve(t);
        let mut mesh = egui::Mesh::default();
        let (c0, c1, c2, c3) = match grad.direction {
            Direction::Vertical => (from, from, to, to),
            Direction::Horizontal => (from, to, to, from),
        };
        let uv = egui::epaint::WHITE_UV;
        mesh.colored_vertex(rect.left_top(), c0);
        mesh.colored_vertex(rect.right_top(), c1);
        mesh.colored_vertex(rect.right_bottom(), c2);
        mesh.colored_vertex(rect.left_bottom(), c3);
        for v in &mut mesh.vertices {
            v.uv = uv;
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        ui.painter().set(idx, egui::Shape::mesh(mesh));
    }
    paint_edge_borders(ui.painter(), rect, style, t);
}

fn render_scroll_generic(
    ui: &mut Ui,
    t: &Theme,
    direction: Direction,
    style: &NodeStyle,
    children: &[Node],
    render_child: &mut dyn FnMut(&mut Ui, &Node),
) {
    let gap = style.gap.unwrap_or(0.0);
    let area = match direction {
        Direction::Vertical => egui::ScrollArea::vertical(),
        Direction::Horizontal => egui::ScrollArea::horizontal(),
    };
    area.auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(gap, gap);
        let frame = frame_from_style(style, t, Color32::TRANSPARENT);
        frame.show(ui, |ui| {
            for c in children {
                render_child(ui, c);
            }
        });
    });
}

// ── Library Renderer ────────────────────────────────────────────────────────

/// Callback trait implemented by `builtin::library::Library` so the generic
/// tree renderer can render library components without owning library state.
pub trait LibraryHost {
    fn is_detail_open(&self) -> bool;
    fn render_detail(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx);
    fn handle_library_action(&mut self, action: &str, ctx: &mut Ctx);
    fn render_search(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, placeholder: Option<&str>, show_filter_button: bool, style: &NodeStyle);
    fn render_filters(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, variant: &str, style: &NodeStyle);
    fn render_list(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, row_height: f32, icon_size: f32, group_headers: bool, show_counts: bool, style: &NodeStyle);
    #[allow(clippy::too_many_arguments)]
    fn render_whats_new(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, title: Option<&str>, card_width: f32, card_height: f32, card_radius: u8, date_above: bool, max_items: usize, gap: f32, style: &NodeStyle);
    #[allow(clippy::too_many_arguments)]
    fn render_recent_games(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, title: Option<&str>, featured_first: bool, featured_width: f32, capsule_width: f32, capsule_height: f32, card_radius: u8, gap: f32, style: &NodeStyle);
    #[allow(clippy::too_many_arguments)]
    fn render_all_games(&mut self, ui: &mut Ui, t: &Theme, ctx: &mut Ctx, title: Option<&str>, show_count: bool, show_sort: bool, card_aspect: &str, card_width: f32, card_height: f32, card_radius: u8, gap: f32, style: &NodeStyle);
}

struct LibRenderCtx<'a, 'b> {
    host: &'a mut dyn LibraryHost,
    t: Theme,
    ctx: &'a mut Ctx<'b>,
    panel_seq: usize,
}

pub fn render_library(host: &mut dyn LibraryHost, nodes: &[Node], ui: &mut Ui, t: &Theme, ctx: &mut Ctx) {
    let mut lctx = LibRenderCtx {
        host,
        t: t.clone(),
        ctx,
        panel_seq: 0,
    };
    for node in nodes {
        render_lib_node(&mut lctx, ui, node);
    }
}

fn render_lib_node(lctx: &mut LibRenderCtx<'_, '_>, ui: &mut Ui, node: &Node) {
    let t = lctx.t.clone();
    match node {
        Node::Panel { side, size, resizable, style, children, .. } => {
            lctx.panel_seq += 1;
            let pid = format!("theme-lib-panel-{}", lctx.panel_seq);
            let default_fill = match side {
                PanelSide::Center => Color32::TRANSPARENT,
                _ => t.chrome,
            };
            let fill = style.bg.map(|c| c.resolve(&t)).unwrap_or(default_fill);
            let padding = style.padding.map(|e| e.to_egui_margin()).unwrap_or(Margin::ZERO);
            let frame = egui::Frame::new().fill(fill).inner_margin(Margin::ZERO);

            let render_body = |lctx: &mut LibRenderCtx<'_, '_>, ui: &mut Ui| {
                let full_rect = ui.max_rect();
                if let Some(grad) = &style.bg_gradient {
                    paint_gradient(ui.painter(), full_rect, grad, &t);
                }
                egui::Frame::new().inner_margin(padding).show(ui, |ui| {
                    if let Some(gap) = style.gap {
                        ui.spacing_mut().item_spacing = vec2(gap, gap);
                    }
                    for child in children {
                        render_lib_node(lctx, ui, child);
                    }
                });
                paint_edge_borders(ui.painter(), full_rect, style, &t);
            };

            match side {
                PanelSide::Top => {
                    let mut p = egui::Panel::top(pid).resizable(*resizable).frame(frame);
                    if let Some(sz) = size {
                        p = if *resizable { p.default_size(*sz) } else { p.exact_size(*sz) };
                    }
                    p.show(ui, |ui| render_body(lctx, ui));
                }
                PanelSide::Bottom => {
                    let mut p = egui::Panel::bottom(pid).resizable(*resizable).frame(frame);
                    if let Some(sz) = size {
                        p = if *resizable { p.default_size(*sz) } else { p.exact_size(*sz) };
                    }
                    p.show(ui, |ui| render_body(lctx, ui));
                }
                PanelSide::Left => {
                    let mut p = egui::Panel::left(pid).resizable(*resizable).frame(frame);
                    if let Some(sz) = size {
                        p = if *resizable { p.default_size(*sz) } else { p.exact_size(*sz) };
                    }
                    p.show(ui, |ui| render_body(lctx, ui));
                }
                PanelSide::Right => {
                    let mut p = egui::Panel::right(pid).resizable(*resizable).frame(frame);
                    if let Some(sz) = size {
                        p = if *resizable { p.default_size(*sz) } else { p.exact_size(*sz) };
                    }
                    p.show(ui, |ui| render_body(lctx, ui));
                }
                PanelSide::Center => {
                    egui::CentralPanel::default().frame(frame).show(ui, |ui| render_body(lctx, ui));
                }
            }
        }

        Node::Row { justify, align, wrap, style, children } => {
            render_row_generic(ui, &t, *justify, *align, *wrap, style, children, &mut |ui, child| {
                render_lib_node(lctx, ui, child);
            });
        }

        Node::Col { align, style, children } => {
            render_col_generic(ui, &t, *align, style, children, &mut |ui, child| {
                render_lib_node(lctx, ui, child);
            });
        }

        Node::Box { style, children } => {
            render_box_generic(ui, &t, style, children, &mut |ui, child| {
                render_lib_node(lctx, ui, child);
            });
        }

        Node::Scroll { direction, style, children } => {
            render_scroll_generic(ui, &t, *direction, style, children, &mut |ui, child| {
                render_lib_node(lctx, ui, child);
            });
        }

        Node::Spacer { size, flex, .. } => {
            if !flex {
                ui.add_space(size.unwrap_or(8.0));
            }
        }

        Node::Divider { orientation, color, thickness, .. } => {
            let c = color.map(|cr| cr.resolve(&t)).unwrap_or(t.border);
            match orientation {
                Direction::Horizontal => {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), (*thickness).max(1.0)), Sense::hover());
                    ui.painter().hline(r.x_range(), r.center().y, Stroke::new(*thickness, c));
                }
                Direction::Vertical => {
                    let (r, _) = ui.allocate_exact_size(vec2((*thickness).max(1.0), ui.available_height().min(28.0)), Sense::hover());
                    ui.painter().vline(r.center().x, r.y_range(), Stroke::new(*thickness, c));
                }
            }
        }

        Node::Text { value, style } => {
            let mut text = value.clone();
            if style.uppercase.unwrap_or(false) {
                text = text.to_uppercase();
            }
            let color = style.color.map(|c| c.resolve(&t)).unwrap_or(t.text);
            let font = font_for(style.font_weight, style.font_size.unwrap_or(13.0));
            ui.label(egui::RichText::new(text).font(font).color(color));
        }

        Node::Button { label, icon, action, style } => {
            let resp = render_styled_button(ui, &t, label, icon.as_deref(), style);
            if resp.clicked() {
                lctx.host.handle_library_action(action, lctx.ctx);
            }
        }

        Node::LibrarySearch { placeholder, show_filter_button, style } => {
            lctx.host.render_search(ui, &t, lctx.ctx, placeholder.as_deref(), *show_filter_button, style);
        }

        Node::LibraryFilters { variant, style } => {
            lctx.host.render_filters(ui, &t, lctx.ctx, variant, style);
        }

        Node::LibraryList { row_height, icon_size, group_headers, show_counts, style } => {
            lctx.host.render_list(ui, &t, lctx.ctx, *row_height, *icon_size, *group_headers, *show_counts, style);
        }

        Node::WhatsNewShelf { title, card_width, card_height, card_radius, date_above, max_items, gap, style } => {
            lctx.host.render_whats_new(
                ui,
                &t,
                lctx.ctx,
                title.as_deref(),
                *card_width,
                *card_height,
                *card_radius,
                *date_above,
                *max_items,
                *gap,
                style,
            );
        }

        Node::RecentGamesShelf {
            title,
            featured_first,
            featured_width,
            capsule_width,
            capsule_height,
            card_radius,
            gap,
            style,
        } => {
            lctx.host.render_recent_games(
                ui,
                &t,
                lctx.ctx,
                title.as_deref(),
                *featured_first,
                *featured_width,
                *capsule_width,
                *capsule_height,
                *card_radius,
                *gap,
                style,
            );
        }

        Node::AllGamesGrid {
            title,
            show_count,
            show_sort,
            card_aspect,
            card_width,
            card_height,
            card_radius,
            gap,
            style,
        } => {
            lctx.host.render_all_games(
                ui,
                &t,
                lctx.ctx,
                title.as_deref(),
                *show_count,
                *show_sort,
                card_aspect,
                *card_width,
                *card_height,
                *card_radius,
                *gap,
                style,
            );
        }

        Node::LibraryHomeOrDetail { children, .. } => {
            if lctx.host.is_detail_open() {
                lctx.host.render_detail(ui, &t, lctx.ctx);
            } else {
                for child in children {
                    render_lib_node(lctx, ui, child);
                }
            }
        }

        // Shell-only nodes inside a library tree are no-ops.
        Node::Logo { .. }
        | Node::Menu { .. }
        | Node::NavTabs { .. }
        | Node::BackForward { .. }
        | Node::WindowControls { .. }
        | Node::Account { .. }
        | Node::Wallet { .. }
        | Node::Notifications { .. }
        | Node::FriendsButton { .. }
        | Node::DownloadsBar { .. }
        | Node::FriendsDrawer { .. }
        | Node::PageContent { .. } => {}
    }
}
