use std::collections::BTreeMap;

use themes::{
    DrawCmd, LayoutTree, Node, OptionValue, PanelSide, ThemeManifest, ThemeScript, parse_hex_color,
};

const MANIFEST: &str = include_str!("../../../docs/examples/steam-replica/theme.toml");
const LAYOUT: &str = include_str!("../../../docs/examples/steam-replica/layout.toml");
const SCRIPT: &str = include_str!("../../../docs/examples/steam-replica/theme.rhai");

fn walk<'a>(nodes: &'a [Node], result: &mut Vec<&'a Node>) {
    for node in nodes {
        result.push(node);
        match node {
            Node::Panel { children, .. }
            | Node::Row { children, .. }
            | Node::Col { children, .. }
            | Node::Box { children, .. }
            | Node::Scroll { children, .. }
            | Node::LibraryHomeOrDetail { children, .. } => walk(children, result),
            _ => {}
        }
    }
}

#[test]
fn all_steam_eras_reach_header_sidebar_and_library_gradients() {
    let manifest = ThemeManifest::parse(MANIFEST).unwrap();
    let layout = LayoutTree::parse(LAYOUT).unwrap();
    let mut nodes = Vec::new();
    walk(&layout.shell, &mut nodes);
    walk(&layout.library, &mut nodes);
    assert!(
        nodes
            .iter()
            .any(|node| matches!(node, Node::NavTabs { .. }))
    );
    assert!(nodes.iter().any(|node| matches!(
        node,
        Node::Panel {
            side: PanelSide::Left,
            ..
        }
    )));
    assert_eq!(
        nodes
            .iter()
            .filter(|node| matches!(node, Node::Box { style, .. } if style.bg_gradient.is_some()))
            .count(),
        2
    );
    for era in ["modern_2023", "classic_2018", "vgui_2010"] {
        let theme = manifest
            .resolve_tokens(&BTreeMap::from([(
                "steam_era".into(),
                OptionValue::Text(era.into()),
            )]))
            .theme;
        for node in &nodes {
            match node {
                Node::NavTabs { style, .. } => {
                    assert_eq!(
                        style.active_color.unwrap().resolve(&theme),
                        theme.accent,
                        "{era} navigation"
                    );
                    assert_eq!(
                        style.color.unwrap().resolve(&theme),
                        theme.text,
                        "{era} navigation text"
                    );
                }
                Node::Panel {
                    side: PanelSide::Left,
                    style,
                    ..
                } => {
                    assert_eq!(style.bg.unwrap().resolve(&theme), theme.bg, "{era} sidebar");
                    assert_eq!(
                        style.border_right.as_ref().unwrap().color.resolve(&theme),
                        theme.border
                    );
                }
                Node::Panel {
                    side: PanelSide::Top,
                    style,
                    ..
                } => {
                    assert_eq!(
                        style.border_bottom.as_ref().unwrap().color.resolve(&theme),
                        theme.border,
                        "{era} header border"
                    );
                }
                Node::Notifications {
                    highlight_unread_bg,
                    style,
                    ..
                } => {
                    assert_eq!(
                        highlight_unread_bg.unwrap().resolve(&theme),
                        theme.discount,
                        "{era} unread badge"
                    );
                    assert_eq!(
                        style.bg.unwrap().resolve(&theme),
                        theme.bg,
                        "{era} notification surface"
                    );
                }
                Node::FriendsButton { style, .. } => {
                    assert_eq!(
                        style.bg.unwrap().resolve(&theme),
                        theme.accent,
                        "{era} friends accent"
                    );
                    assert_eq!(
                        style.color.unwrap().resolve(&theme),
                        theme.on_accent,
                        "{era} friends contrast"
                    );
                }
                Node::Box { style, .. } if style.bg_gradient.is_some() => {
                    let gradient = style.bg_gradient.as_ref().unwrap();
                    assert!(
                        [theme.raised, theme.surface].contains(&gradient.from.resolve(&theme)),
                        "{era} shelf top"
                    );
                    assert!(
                        [theme.chrome, theme.bg].contains(&gradient.to.resolve(&theme)),
                        "{era} shelf bottom"
                    );
                }
                _ => {}
            }
        }
    }
}

#[test]
fn steam_visual_hooks_follow_roles_even_with_a_custom_accent() {
    let manifest = ThemeManifest::parse(MANIFEST).unwrap();
    for (era, from, to, border) in [
        ("modern_2023", "#2B303A40", "#171D2560", "#14171D"),
        ("classic_2018", "#21324540", "#171A2160", "#3B5774"),
        ("vgui_2010", "#2D302B40", "#1E201D60", "#464B42"),
    ] {
        let mut theme = manifest
            .resolve_tokens(&BTreeMap::from([(
                "steam_era".into(),
                OptionValue::Text(era.into()),
            )]))
            .theme;
        let mut script = ThemeScript::compile(SCRIPT).unwrap();
        let before = script.paint_background(800.0, 600.0, 0.0, &theme);
        assert!(
            matches!(&before[..], [DrawCmd::Gradient { from: actual_from, to: actual_to, .. }] if *actual_from == parse_hex_color(from).unwrap() && *actual_to == parse_hex_color(to).unwrap()),
            "{era} gradient"
        );
        assert!(
            matches!(&script.paint_chrome(800.0, 80.0, 0.0, &theme)[..], [DrawCmd::Line { color, .. }] if *color == parse_hex_color(border).unwrap()),
            "{era} chrome"
        );
        theme.accent = parse_hex_color("#E27BB0").unwrap();
        assert_eq!(
            script.paint_background(800.0, 600.0, 0.0, &theme),
            before,
            "{era} custom accent must preserve neutral surfaces"
        );
        assert!(script.error().is_none());
    }
}
