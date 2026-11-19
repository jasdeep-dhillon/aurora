// Steam Desktop Replica visual hooks.
// Subtle neutral-cool vertical slate gradient on the main window canvas
// and a crisp 1px dark chrome hairline.

fn paint_background(w, h, _time, theme) {
    let from_color = if theme.accent == "#8B9B64" {
        // VGUI 2010 Olive subtle top sheen
        "#2F322C34"
    } else if theme.accent == "#66C0F4" {
        // Classic 2018 deeper blue-slate wash
        "#23374D3C"
    } else {
        // Modern 2023+ authentic neutral-cool slate gradient
        "#262B3540"
    };

    let to_color = if theme.accent == "#8B9B64" {
        "#1E201D38"
    } else if theme.accent == "#66C0F4" {
        "#151F2C3C"
    } else {
        "#1E222A40"
    };

    [
        #{
            type: "gradient",
            x: 0.0,
            y: 0.0,
            w: w,
            h: h,
            from: from_color,
            to: to_color,
            horizontal: false,
        },
    ]
}

fn paint_chrome(w, h, _time, _theme) {
    [
        #{
            type: "line",
            x1: 0.0,
            y1: h - 1.0,
            x2: w,
            y2: h - 1.0,
            width: 1.0,
            color: "#0E1116",
        },
    ]
}

fn paint_card_overlay(w, h, hovered, _installed, theme) {
    if !hovered {
        return [];
    }

    [
        #{
            type: "rect_stroke",
            x: 0.0,
            y: 0.0,
            w: w,
            h: h,
            radius: 0.0,
            width: 1.0,
            color: theme.online + "99",
        },
        #{
            type: "gradient",
            x: 0.0,
            y: 0.0,
            w: w,
            h: h * 0.20,
            from: "#FFFFFF14",
            to: "#FFFFFF00",
            horizontal: false,
        },
    ]
}
