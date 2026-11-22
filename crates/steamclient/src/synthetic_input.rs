//! Translate legacy controller bindings into keyboard and mouse input for the
//! foreground game. Keep track of injected presses so they can be released.
use crate::{legacy::Sample, overlay, pads};
use std::{collections::BTreeSet, mem::size_of, thread, time::Duration};
use vdf::controller::{Control, LegacyMap, Output};
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC_EX, MOUSE_EVENT_FLAGS,
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
        MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
        MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, MapVirtualKeyW, SendInput, VIRTUAL_KEY,
    },
    WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Button {
    Key(u16),
    Mouse(u8),
}

#[derive(Default)]
struct Frame {
    buttons: BTreeSet<Button>,
    wheel: BTreeSet<(Control, i8)>,
    motion: [f32; 2],
}

impl Frame {
    fn from_sample(sample: Sample, layout: &LegacyMap) -> Self {
        let mut frame = Self::default();
        for (&control, outputs) in &layout.0 {
            let pressed = sample.pressed(control);
            for output in outputs {
                match *output {
                    Output::Key(key) if pressed => {
                        frame.buttons.insert(Button::Key(key));
                    }
                    Output::MouseButton(button) if pressed => {
                        frame.buttons.insert(Button::Mouse(button));
                    }
                    Output::Wheel(direction) if pressed => {
                        frame.wheel.insert((control, direction));
                    }
                    Output::MouseMove if control.vector() => {
                        let [x, y] = sample.value(control);
                        let deadzone = if control == Control::Gyro { 0.02 } else { 0.15 };
                        if x.abs().max(y.abs()) > deadzone {
                            frame.motion[0] += x;
                            frame.motion[1] -= y;
                        }
                    }
                    _ => {}
                }
            }
        }
        frame
    }
}

fn mouse(flags: MOUSE_EVENT_FLAGS, data: u32, dx: i32, dy: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                ..Default::default()
            },
        },
    }
}

impl Button {
    fn event(self, down: bool) -> Option<INPUT> {
        match self {
            Button::Key(key) => {
                let scan = unsafe { MapVirtualKeyW(u32::from(key), MAPVK_VK_TO_VSC_EX) };
                let mut flags = KEYBD_EVENT_FLAGS::default();
                if scan >> 8 == 0xe0 {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                if !down {
                    flags |= KEYEVENTF_KEYUP;
                }
                Some(INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VIRTUAL_KEY(key),
                            dwFlags: flags,
                            ..Default::default()
                        },
                    },
                })
            }
            Button::Mouse(button) => {
                let (press, release, data) = match button {
                    1 => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, 0),
                    2 => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, 0),
                    3 => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, 0),
                    4 | 5 => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, u32::from(button - 3)),
                    _ => return None,
                };
                Some(mouse(if down { press } else { release }, data, 0, 0))
            }
        }
    }
}

fn send(event: INPUT) -> bool {
    unsafe { SendInput(&[event], size_of::<INPUT>() as i32) == 1 }
}

fn game_has_focus() -> bool {
    let mut foreground_pid = 0;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut foreground_pid)) };
    foreground_pid == std::process::id() && !overlay::is_open()
}

fn bindings() -> (u64, LegacyMap) {
    let (revision, mut layout) = pads::synthetic_layout();
    for outputs in layout.0.values_mut() {
        outputs.retain(|output| {
            matches!(
                output,
                Output::Key(_) | Output::MouseButton(_) | Output::Wheel(_) | Output::MouseMove
            )
        });
    }
    layout.0.retain(|_, outputs| !outputs.is_empty());
    (revision, layout)
}

fn run() {
    let (mut revision, mut layout) = bindings();
    let mut held = BTreeSet::<Button>::new();
    let mut wheel = BTreeSet::new();
    let mut motion_remainder = [0.0; 2];
    loop {
        if pads::synthetic_revision() != revision {
            (revision, layout) = bindings();
        }
        let active = !layout.0.is_empty() && game_has_focus();
        let frame = if active {
            pads::synthetic_pad().map(|sample| Frame::from_sample(sample, &layout))
        } else {
            None
        };
        if frame.is_none() {
            motion_remainder = [0.0; 2];
        }
        let frame = frame.unwrap_or_default();
        held.retain(|button| {
            frame.buttons.contains(button) || !button.event(false).is_some_and(send)
        });
        for button in frame.buttons {
            if !held.contains(&button) && game_has_focus() && button.event(true).is_some_and(send) {
                held.insert(button);
            }
        }
        for (_, direction) in frame.wheel.difference(&wheel) {
            if game_has_focus() {
                send(mouse(
                    MOUSEEVENTF_WHEEL,
                    (i32::from(*direction) * 120) as u32,
                    0,
                    0,
                ));
            }
        }
        wheel = frame.wheel;

        let [dx, dy] = std::array::from_fn(|axis| {
            let pixels = frame.motion[axis].clamp(-1.0, 1.0) * 12.0 + motion_remainder[axis];
            motion_remainder[axis] = pixels.fract();
            pixels as i32
        });
        if (dx != 0 || dy != 0) && game_has_focus() {
            send(mouse(MOUSEEVENTF_MOVE, 0, dx, dy));
        }
        thread::sleep(Duration::from_millis(if active { 8 } else { 50 }));
    }
}

pub(crate) fn start() {
    if let Err(error) = thread::Builder::new()
        .name("controller-kbm".into())
        .spawn(run)
    {
        crate::log!("input: could not start controller keyboard and mouse worker: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pads::Pad;

    #[test]
    fn keeps_shared_outputs_held_until_the_last_source_is_released() {
        let layout = LegacyMap(
            [
                (Control::A, vec![Output::Key(b'W'.into())]),
                (
                    Control::B,
                    vec![Output::Key(b'W'.into()), Output::Wheel(-1)],
                ),
                (Control::RightStick, vec![Output::MouseMove]),
            ]
            .into_iter()
            .collect(),
        );
        let frame = |buttons| {
            Frame::from_sample(
                Sample {
                    pad: Pad {
                        buttons,
                        ry: i16::MAX,
                        ..Pad::default()
                    },
                    ..Sample::default()
                },
                &layout,
            )
        };
        let both = frame(Control::A.button() | Control::B.button());
        let only_b = frame(Control::B.button());
        assert!(both.buttons == BTreeSet::from([Button::Key(b'W'.into())]));
        assert!(both.buttons == only_b.buttons);
        assert_eq!(only_b.wheel, BTreeSet::from([(Control::B, -1)]));
        assert_eq!(both.motion, [0.0, -1.0]);
        assert!(frame(0).buttons.is_empty());
    }
}
