// Steam Desktop visual hooks. Use semantic roles so every era and custom
// accent keeps the same neutral depth, without guessing the era from a color.

fn paint_background(w, h, _time, theme) {
    [
        #{
            type: "gradient",
            x: 0.0,
            y: 0.0,
            w: w,
            h: h,
            from: theme.surface + "40",
            to: theme.chrome + "60",
            horizontal: false,
        },
    ]
}

fn paint_chrome(w, h, _time, theme) {
    [
        #{
            type: "line",
            x1: 0.0,
            y1: h - 1.0,
            x2: w,
            y2: h - 1.0,
            width: 1.0,
            color: theme.border,
        },
    ]
}
