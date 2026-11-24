//! Shared on-screen keyboard for Big Picture & Steam Deck Game Mode:
//! English + Cyrillic alphabets, Shift (uppercase), Symbols layer, Clear,
//! and controller shortcuts (X = Delete, Y = Space, LT = Shift, RT = Go).

use super::focus::Place;
use super::{BigPicture, Frame, paint};
use egui::{Align2, Id, Rect, Ui, pos2, vec2};
use i18n::t;
use ui::widgets as w;

const KEYS_EN: [&str; 4] = ["1234567890", "qwertyuiop", "asdfghjkl'", "zxcvbnm-:."];
const KEYS_EN_SHIFT: [&str; 4] = ["!@#$%^&*()", "QWERTYUIOP", "ASDFGHJKL\"", "ZXCVBNM_?!"];
const KEYS_RU: [&str; 4] = ["1234567890-", "йцукенгшщзх", "фывапролджэ", "ячсмитьбюъё"];
const KEYS_RU_SHIFT: [&str; 4] = ["!\"№;%:?*()_", "ЙЦУКЕНГШЩЗХ", "ФЫВАПРОЛДЖЭ", "ЯЧСМИТЬБЮЪЁ"];
const KEYS_SYM: [&str; 4] = ["!@#$%^&*()", "-_=+[]{};:", "/?,.<>'\"~`", "|\\€£¥°•·+*"];

/// Keyboard mode state held on `BigPicture`.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct OskState {
    pub cyrillic: bool,
    pub shift: bool,
    pub symbols: bool,
}

impl OskState {
    pub fn rows(self) -> [&'static str; 4] {
        if self.symbols {
            KEYS_SYM
        } else if self.cyrillic {
            if self.shift { KEYS_RU_SHIFT } else { KEYS_RU }
        } else if self.shift {
            KEYS_EN_SHIFT
        } else {
            KEYS_EN
        }
    }

    pub fn max_cols(self) -> usize {
        self.rows().iter().map(|r| r.chars().count()).max().unwrap_or(10)
    }
}

/// A key pressed on the on-screen keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Del,
    Clear,
    Go,
}

/// Draw the on-screen keyboard from `y` down; advances `y` past its bottom.
/// Fits cleanly within `max_width` on both 1080p TVs and 1280×800 handhelds.
pub fn draw(
    bp: &mut BigPicture,
    ui: &mut Ui,
    f: &Frame,
    left: f32,
    max_width: f32,
    y: &mut f32,
    go: &str,
) -> Option<Key> {
    let s = f.s;
    if f.lt {
        bp.osk.shift = !bp.osk.shift;
    }
    let mut out = if f.rt { Some(Key::Go) } else { None };

    let rows = bp.osk.rows();
    let cols = bp.osk.max_cols() as f32;
    let gap = 8.0 * s;
    let key = ((max_width - (cols - 1.0) * gap) / cols).clamp(48.0 * s, 72.0 * s);
    let unit = key + gap;
    let row_w = cols * unit - gap;
    let start_x = left + ((max_width - row_w) * 0.5).max(0.0);

    for (ri, row) in rows.iter().enumerate() {
        let count = row.chars().count() as f32;
        let rx = start_x + ((cols - count) * 0.5 * unit).max(0.0);
        for (ci, ch) in row.chars().enumerate() {
            let r = Rect::from_min_size(pos2(rx + ci as f32 * unit, *y), vec2(key, key));
            let label = ch.to_string();
            if key_cap(bp, ui, f, Id::unique(("bp-key", ri, ci)), r, &label, false, false) {
                out = Some(Key::Char(ch));
            }
        }
        *y += unit;
    }

    // Bottom action row scaled proportionally to fit `row_w`.
    let shift_label = if bp.osk.shift { "SHIFT" } else { "Shift" };
    let sym_label = if bp.osk.symbols { "ABC" } else { "!@#" };
    let lang_label = if bp.osk.cyrillic { "ABC" } else { "АБВ" };
    let specials: [(f32, &str, &str, bool, bool); 7] = [
        (1.4, "shift", shift_label, false, bp.osk.shift),
        (1.3, "sym", sym_label, false, bp.osk.symbols),
        (1.3, "lang", lang_label, false, bp.osk.cyrillic && !bp.osk.symbols),
        (2.4, " ", t!("Space"), false, false),
        (1.4, "del", t!("Delete"), false, false),
        (1.2, "clear", t!("Clear"), false, false),
        (2.0, "go", go, true, false),
    ];
    let weight_sum: f32 = specials.iter().map(|x| x.0).sum();
    let avail = row_w - gap * (specials.len() - 1) as f32;
    let mut x = start_x;
    for (i, (weight, what, label, accent, active)) in specials.iter().enumerate() {
        let wdt = (avail * (*weight / weight_sum)).max(key);
        let r = Rect::from_min_size(pos2(x, *y), vec2(wdt, key));
        if key_cap(bp, ui, f, Id::unique(("bp-key-special", i)), r, label, *accent, *active) {
            match *what {
                " " => out = Some(Key::Char(' ')),
                "del" => out = Some(Key::Del),
                "clear" => out = Some(Key::Clear),
                "shift" => {
                    bp.osk.symbols = false;
                    bp.osk.shift = !bp.osk.shift;
                }
                "sym" => {
                    bp.osk.symbols = !bp.osk.symbols;
                }
                "lang" => {
                    bp.osk.symbols = false;
                    bp.osk.cyrillic = !bp.osk.cyrillic;
                }
                _ => out = Some(Key::Go),
            }
        }
        x += wdt + gap;
    }
    *y += unit;
    out
}

/// Height the keyboard occupies at scale `s` and `max_width` (for chat slide-up animation).
pub fn panel_height(s: f32, max_width: f32, state: OskState) -> f32 {
    let cols = state.max_cols() as f32;
    let gap = 8.0 * s;
    let key = ((max_width - (cols - 1.0) * gap) / cols).clamp(48.0 * s, 72.0 * s);
    5.0 * (key + gap) + 34.0 * s
}

fn key_cap(
    bp: &mut BigPicture,
    ui: &Ui,
    f: &Frame,
    id: Id,
    r: Rect,
    label: &str,
    accent: bool,
    active: bool,
) -> bool {
    let s = f.s;
    let hit = bp.focus.item(ui, id, r, ui.clip_rect(), Place::Page);
    if hit.activated {
        bp.key_pop = Some((id, f.now));
    }
    let pop = bp
        .key_pop
        .filter(|(k, _)| *k == id)
        .map(|(_, at)| ((f.now - at) / 0.16) as f32)
        .filter(|e| *e < 1.0);
    let bump = pop.map(|e| (e * std::f32::consts::PI).sin() * 0.1).unwrap_or(0.0);
    if pop.is_some() {
        ui.ctx().request_repaint();
    }
    let shown = paint::grow(r, 1.0 + 0.05 * hit.glow - bump);
    let base = if accent {
        f.t.accent
    } else if active {
        f.t.accent.gamma_multiply(0.45)
    } else {
        f.t.raised
    };
    let fill = w::mix(base, f.t.accent, hit.glow * 0.75);
    ui.painter().rect_filled(shown, paint::radius(10.0 * s), fill);
    let size = if label.chars().count() == 1 { 28.0 } else { 18.0 };
    let text_color = if accent { f.t.on_accent } else { f.t.text };
    ui.painter().text(shown.center(), Align2::CENTER_CENTER, label, w::font_semibold(size * s), text_color);
    if hit.focused {
        bp.focus.ring_around(shown, 10.0 * s);
    }
    hit.activated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osk_layers_have_four_non_empty_rows() {
        for state in [
            OskState { cyrillic: false, shift: false, symbols: false },
            OskState { cyrillic: false, shift: true, symbols: false },
            OskState { cyrillic: true, shift: false, symbols: false },
            OskState { cyrillic: true, shift: true, symbols: false },
            OskState { cyrillic: false, shift: false, symbols: true },
        ] {
            let rows = state.rows();
            assert_eq!(rows.len(), 4);
            assert!(rows.iter().all(|r| r.chars().count() >= 10));
            assert!(state.max_cols() >= 10);
        }
        assert!(OskState { cyrillic: false, shift: true, symbols: false }.rows()[1].starts_with('Q'));
        assert!(OskState { cyrillic: false, shift: false, symbols: true }.rows()[0].starts_with('!'));
    }
}
