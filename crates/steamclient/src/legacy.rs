//! Legacy gamepad translation. Always read the original pad so swaps and
//! mappings between several inputs do not depend on iteration order.
use crate::pads::Pad;
use vdf::controller::{Control, LegacyMap, Output};

#[derive(Clone, Copy, Default)]
pub(crate) struct Sample {
    pub pad: Pad,
    pub paddles: u8,
    pub gyro: [f32; 3],
}

impl Sample {
    pub fn value(self, control: Control) -> [f32; 2] {
        use Control::*;
        let p = self.pad;
        let stick = |v: i16| (v as f32 / 32767.0).max(-1.0);
        let axis = |v: i16| if v > 16384 { stick(v) } else { 0.0 };
        let scalar = match control {
            LT => p.lt as f32 / 255.0,
            RT => p.rt as f32 / 255.0,
            LeftStick => return [stick(p.lx), stick(p.ly)],
            RightStick => return [stick(p.rx), stick(p.ry)],
            Gyro => return [
                (-self.gyro[1] / 5.0).clamp(-1.0, 1.0),
                (self.gyro[0] / 5.0).clamp(-1.0, 1.0),
            ],
            LeftStickUp => axis(p.ly), LeftStickDown => axis(p.ly.saturating_neg()),
            LeftStickLeft => axis(p.lx.saturating_neg()), LeftStickRight => axis(p.lx),
            RightStickUp => axis(p.ry), RightStickDown => axis(p.ry.saturating_neg()),
            RightStickLeft => axis(p.rx.saturating_neg()), RightStickRight => axis(p.rx),
            L4 => f32::from(self.paddles & 2 != 0), R4 => f32::from(self.paddles & 1 != 0),
            L5 => f32::from(self.paddles & 8 != 0), R5 => f32::from(self.paddles & 4 != 0),
            _ => f32::from(p.buttons & control.button() != 0),
        };
        [scalar, 0.0]
    }

    pub fn pressed(self, control: Control) -> bool {
        let [x, y] = self.value(control);
        let threshold = if matches!(control, Control::LT | Control::RT) { 0.18 } else { 0.5 };
        x.abs().max(y.abs()) > threshold
    }
}

fn stronger(original: i16, new: i16) -> i16 {
    if i32::from(new).abs() > i32::from(original).abs() { new } else { original }
}

pub(crate) fn remap(sample: Sample, map: &LegacyMap) -> Pad {
    use Control::*;
    let mut game = sample.pad;
    for input in map.0.keys() {
        game.buttons &= !input.button();
        match input {
            LT => game.lt = 0, RT => game.rt = 0,
            LeftStick => { game.lx = 0; game.ly = 0; }
            RightStick => { game.rx = 0; game.ry = 0; }
            LeftStickUp if game.ly > 0 => game.ly = 0,
            LeftStickDown if game.ly < 0 => game.ly = 0,
            LeftStickLeft if game.lx < 0 => game.lx = 0,
            LeftStickRight if game.lx > 0 => game.lx = 0,
            RightStickUp if game.ry > 0 => game.ry = 0,
            RightStickDown if game.ry < 0 => game.ry = 0,
            RightStickLeft if game.rx < 0 => game.rx = 0,
            RightStickRight if game.rx > 0 => game.rx = 0,
            _ => {}
        }
    }
    for (&input, outputs) in &map.0 {
        let [x, y] = sample.value(input);
        let axis = |v: f32| (v * 32767.0) as i16;
        let trigger = || (x.max(0.0) * 255.0) as u8;
        for output in outputs {
            let Output::Pad(target) = output else { continue };
            match target {
                LT => game.lt = game.lt.max(trigger()), RT => game.rt = game.rt.max(trigger()),
                LeftStick => { game.lx = stronger(game.lx, axis(x)); game.ly = stronger(game.ly, axis(y)); }
                RightStick => { game.rx = stronger(game.rx, axis(x)); game.ry = stronger(game.ry, axis(y)); }
                button if sample.pressed(input) => game.buttons |= button.button(),
                _ => {}
            }
        }
    }
    game
}

#[cfg(test)]
mod tests {
    use super::*;
    use Control::*;

    #[test]
    fn swaps_original_inputs_and_preserves_unmapped_controls() {
        let map = LegacyMap([(A, vec![Output::Pad(B)]), (B, vec![Output::Pad(A)]),
            (LT, vec![Output::Pad(RT)]), (X, vec![Output::Unassigned])].into_iter().collect());
        let pad = Pad { buttons: A.button() | Y.button() | X.button(), lt: 201, lx: 1024, ..Pad::default() };
        let mapped = remap(Sample { pad, ..Sample::default() }, &map);
        assert_eq!(mapped.buttons, B.button() | Y.button());
        assert_eq!((mapped.lt, mapped.rt, mapped.lx), (0, 201, 1024));
    }
}
