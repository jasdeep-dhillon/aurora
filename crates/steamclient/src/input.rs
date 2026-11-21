//! ISteamInput (and the older ISteamController): game actions from Xbox-style
//! pads. The pads are read through XInput (plus, on Windows, the pads
//! XInput can't see, which the app reads with SDL3: see pads.rs); which
//! button does which action comes from the game's Steam Input bindings (a controller_mappings VDF:
//! the player's configuration, the one the game ships, or the game's
//! official one), which Aurora finds like Steam's client does.
//!
//! Without bindings no controller is reported, so the game uses its own
//! controller support, as it would without Steam Input. Gyro, trackpads,
//! mouse-style modes, haptics other than rumble and the binding panel
//! (Steam's overlay configurator) aren't available.

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::ifaces::{This, text};
use crate::pads::{Pad, SteamInputPad};
use crate::pads_shm::{self, PadCalibration};
use crate::protocol::{Reply, Request};
use crate::state::{Bytes, lock};
use crate::vt::Fp;
use std::collections::HashMap;
use std::ffi::c_char;
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub fn imp(name: &str) -> Option<Fp> {
    macro_rules! f {
        ($f:expr) => {
            Some($f as Fp)
        };
    }
    match name {
        "Init" => f!(init),
        "Shutdown" | "SetInputActionManifestFilePath" | "BNewDataAvailable" | "BWaitForData" => f!(yes),
        "RunFrame" => f!(run_frame),
        "GetConnectedControllers" => f!(connected),
        "GetActionSetHandle" => f!(set_handle),
        "ActivateActionSet" => f!(activate_set),
        "GetCurrentActionSet" => f!(current_set),
        "ActivateActionSetLayer" => f!(activate_layer),
        "DeactivateActionSetLayer" => f!(deactivate_layer),
        "DeactivateAllActionSetLayers" => f!(deactivate_layers),
        "GetActiveActionSetLayers" => f!(active_layers),
        "GetDigitalActionHandle" => f!(digital_handle),
        "GetDigitalActionData" => f!(digital_data),
        "GetDigitalActionOrigins" => f!(digital_origins),
        "GetAnalogActionHandle" => f!(analog_handle),
        "GetAnalogActionData" => f!(analog_data),
        "GetAnalogActionOrigins" => f!(analog_origins),
        "GetStringForDigitalActionName" | "GetStringForAnalogActionName" => f!(action_name),
        "GetStringForActionOrigin" => f!(origin_name),
        "TriggerVibration" => f!(vibrate),
        "TriggerVibrationExtended" => f!(vibrate),
        "GetMotionData" => f!(motion_data),
        "GetInputTypeForHandle" => f!(input_type),
        "GetControllerForGamepadIndex" => f!(for_index),
        "GetGamepadIndexForController" => f!(index_of),
        "GetActionOriginFromXboxOrigin" => f!(from_xbox_origin),
        "TranslateActionOrigin" => f!(translate_origin),
        _ => None,
    }
}

extern "C" fn yes() -> bool {
    true
}

// ------------------------------------------------------------------ XInput

const A: u16 = 0x1000;
const B: u16 = 0x2000;
const X: u16 = 0x4000;
const Y: u16 = 0x8000;
const UP: u16 = 0x0001;
const DOWN: u16 = 0x0002;
const LEFT: u16 = 0x0004;
const RIGHT: u16 = 0x0008;
const START: u16 = 0x0010;
const BACK: u16 = 0x0020;
const LS: u16 = 0x0040;
const RS: u16 = 0x0080;
const LB: u16 = 0x0100;
const RB: u16 = 0x0200;

// ---------------------------------------------------------------- bindings

/// One group (a source's mode): its digital inputs and analog game actions.
#[derive(Default, Debug)]
struct Group {
    mode: String,
    /// (input name, actions bound to it).
    inputs: Vec<(String, Vec<String>)>,
    /// Analog actions the whole group drives.
    analog: Vec<String>,
}

#[derive(Default, Debug)]
pub struct Bindings {
    groups: HashMap<String, Group>,
    /// Action set or layer name → (source, group id) when it's active.
    presets: HashMap<String, Vec<(String, String)>>,
    layers: Vec<String>,
}

fn game_actions(input: &vdf::Obj) -> Vec<String> {
    let mut out = Vec::new();
    for (_, act) in input.obj("activators").map(|a| a.iter().collect::<Vec<_>>()).unwrap_or_default() {
        let Some(bindings) = act.as_obj().and_then(|a| a.obj("bindings")) else { continue };
        for (k, v) in bindings.iter() {
            let Some(b) = v.as_str().filter(|_| k == "binding") else { continue };
            let mut words = b.split(',').next().unwrap_or_default().split_whitespace();
            if words.next() == Some("game_action") {
                if let (Some(_set), Some(action)) = (words.next(), words.next()) {
                    out.push(action.to_string());
                }
            }
        }
    }
    out
}

impl Bindings {
    pub fn parse(text: &str) -> Option<Bindings> {
        let doc = vdf::parse(text).ok()?;
        let m = doc.obj("controller_mappings")?;
        let mut b = Bindings::default();
        for (k, v) in m.iter() {
            let Some(o) = v.as_obj() else { continue };
            match k {
                "group" => {
                    let id = o.str("id").unwrap_or_default().to_string();
                    let mut g = Group { mode: o.str("mode").unwrap_or_default().to_string(), ..Default::default() };
                    if let Some(inputs) = o.obj("inputs") {
                        for (name, input) in inputs.iter() {
                            if let Some(input) = input.as_obj() {
                                let acts = game_actions(input);
                                if !acts.is_empty() {
                                    g.inputs.push((name.to_string(), acts));
                                }
                            }
                        }
                    }
                    if let Some(ga) = o.obj("gameactions") {
                        g.analog.extend(ga.iter().filter_map(|(_, a)| a.as_str()).map(str::to_string));
                    }
                    b.groups.insert(id, g);
                }
                "preset" => {
                    let name = o.str("name").unwrap_or_default().to_string();
                    let list = o
                        .obj("group_source_bindings")
                        .map(|g| {
                            g.iter()
                                .filter_map(|(gid, v)| {
                                    let mut w = v.as_str()?.split_whitespace();
                                    let source = w.next()?.to_string();
                                    (w.next() == Some("active")).then(|| (source, gid.to_string()))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    b.presets.insert(name, list);
                }
                "action_layers" => b.layers.extend(o.iter().map(|(n, _)| n.to_string())),
                _ => {}
            }
        }
        (!b.presets.is_empty()).then_some(b)
    }

    /// Active (source, group) for a set plus layers (a layer's sources win).
    fn active(&self, set: &str, layers: &[String]) -> Vec<(&str, &Group)> {
        let mut by_source: HashMap<&str, &Group> = HashMap::new();
        let first = self.presets.get(set).or_else(|| self.presets.values().next());
        for preset in first.into_iter().chain(layers.iter().filter_map(|l| self.presets.get(l))) {
            for (source, gid) in preset {
                if let Some(g) = self.groups.get(gid) {
                    by_source.insert(source.as_str(), g);
                }
            }
        }
        by_source.into_iter().collect()
    }
}

/// Is `input` of a group on `source` pressed?
fn pressed(p: &Pad, source: &str, mode: &str, input: &str, trigger_edge: u8) -> bool {
    let stick = |x: i16, y: i16| (x as f32 / 32767.0, y as f32 / 32767.0);
    let dir = |(x, y): (f32, f32), input: &str| match input {
        "dpad_north" => y > 0.5,
        "dpad_south" => y < -0.5,
        "dpad_east" => x > 0.5,
        "dpad_west" => x < -0.5,
        "edge" => (x * x + y * y).sqrt() > 0.9,
        _ => false,
    };
    let b = |m: u16| p.buttons & m != 0;
    match (source, input) {
        ("button_diamond", "button_a") => b(A),
        ("button_diamond", "button_b") => b(B),
        ("button_diamond", "button_x") => b(X),
        ("button_diamond", "button_y") => b(Y),
        ("dpad", "dpad_north") => b(UP),
        ("dpad", "dpad_south") => b(DOWN),
        ("dpad", "dpad_east") => b(RIGHT),
        ("dpad", "dpad_west") => b(LEFT),
        ("switch", "button_escape") => b(START),
        ("switch", "button_menu") => b(BACK),
        ("switch", "left_bumper") => b(LB),
        ("switch", "right_bumper") => b(RB),
        ("left_joystick", "click") => b(LS),
        ("right_joystick", "click") => b(RS),
        ("left_joystick", i) if mode != "trigger" => dir(stick(p.lx, p.ly), i),
        ("right_joystick", i) => dir(stick(p.rx, p.ry), i),
        ("left_trigger", "click") => p.lt > 216,
        ("right_trigger", "click") => p.rt > 216,
        ("left_trigger", "edge") => p.lt > trigger_edge,
        ("right_trigger", "edge") => p.rt > trigger_edge,
        _ => false,
    }
}

/// (EInputSourceMode, x, y) of a source driving an analog action.
fn analog_value(p: &Pad, source: &str, mode: &str, cal: &PadCalibration, already_calibrated: bool) -> (i32, f32, f32) {
    let (l_dz, r_dz, trig_th) = if already_calibrated {
        (0.0, 0.0, 0.0)
    } else {
        (cal.left_deadzone, cal.right_deadzone, cal.trigger_threshold)
    };
    let v = |x: i16, y: i16, dz: f32| pads_shm::apply_stick_f32((x as f32 / 32767.0).clamp(-1.0, 1.0), (y as f32 / 32767.0).clamp(-1.0, 1.0), dz);
    let (x, y) = match source {
        "left_joystick" => v(p.lx, p.ly, l_dz),
        "right_joystick" => v(p.rx, p.ry, r_dz),
        "left_trigger" => (pads_shm::apply_trigger_f32(p.lt as f32 / 255.0, trig_th), 0.0),
        "right_trigger" => (pads_shm::apply_trigger_f32(p.rt as f32 / 255.0, trig_th), 0.0),
        "dpad" => (((p.buttons & RIGHT != 0) as i32 - (p.buttons & LEFT != 0) as i32) as f32, ((p.buttons & UP != 0) as i32 - (p.buttons & DOWN != 0) as i32) as f32),
        _ => (0.0, 0.0),
    };
    match mode {
        "trigger" => (10, x, 0.0),
        // Mouse-like deltas, roughly what Steam's camera mode gives per frame.
        "joystick_camera" => (8, x * 12.0, -y * 12.0),
        "dpad" => (1, x, y),
        _ => (6, x, y),
    }
}

/// EInputActionOrigin for an Xbox One pad input (Steam Input's numbering).
fn origin(source: &str, input: &str) -> i32 {
    match (source, input) {
        ("button_diamond", "button_a") => 114,
        ("button_diamond", "button_b") => 115,
        ("button_diamond", "button_x") => 116,
        ("button_diamond", "button_y") => 117,
        ("switch", "left_bumper") => 118,
        ("switch", "right_bumper") => 119,
        ("switch", "button_escape") => 124,
        ("switch", "button_menu") => 125,
        ("left_trigger", "click") => 127,
        ("left_trigger", _) => 126,
        ("right_trigger", "click") => 129,
        ("right_trigger", _) => 128,
        ("left_joystick", "click") => 131,
        ("left_joystick", "dpad_north") => 132,
        ("left_joystick", "dpad_south") => 133,
        ("left_joystick", "dpad_west") => 134,
        ("left_joystick", "dpad_east") => 135,
        ("left_joystick", _) => 130,
        ("right_joystick", "click") => 137,
        ("right_joystick", "dpad_north") => 138,
        ("right_joystick", "dpad_south") => 139,
        ("right_joystick", "dpad_west") => 140,
        ("right_joystick", "dpad_east") => 141,
        ("right_joystick", _) => 136,
        ("dpad", "dpad_north") => 142,
        ("dpad", "dpad_south") => 143,
        ("dpad", "dpad_west") => 144,
        ("dpad", "dpad_east") => 145,
        ("dpad", _) => 146,
        _ => 0,
    }
}

fn origin_text(o: i32) -> &'static str {
    match o {
        114 => "A Button",
        115 => "B Button",
        116 => "X Button",
        117 => "Y Button",
        118 => "Left Bumper",
        119 => "Right Bumper",
        124 => "Menu Button",
        125 => "View Button",
        126 => "Left Trigger",
        127 => "Left Trigger Click",
        128 => "Right Trigger",
        129 => "Right Trigger Click",
        130 => "Left Stick",
        131 => "Left Stick Click",
        136 => "Right Stick",
        137 => "Right Stick Click",
        142 => "D-Pad Up",
        143 => "D-Pad Down",
        144 => "D-Pad Left",
        145 => "D-Pad Right",
        146 => "D-Pad",
        132..=135 | 138..=141 => "Stick Direction",
        _ => "",
    }
}

// ------------------------------------------------------------------- state

#[derive(Default)]
struct Input {
    bindings: Option<Bindings>,
    loading: bool,
    disabled_logged: bool,
    /// Interned names: handle = index + 1.
    sets: Vec<String>,
    actions: Vec<String>,
    /// Per controller: active set and layers.
    active: HashMap<u64, (u64, Vec<u64>)>,
    pads: [Option<SteamInputPad>; 4],
    polled: Option<Instant>,
    cal: Option<PadCalibration>,
}

static I: LazyLock<Mutex<Input>> = LazyLock::new(|| Mutex::new(Input::default()));

fn st() -> MutexGuard<'static, Input> {
    I.lock().unwrap_or_else(|e| e.into_inner())
}

fn cal(s: &mut Input) -> PadCalibration {
    *s.cal.get_or_insert_with(crate::pads::calibration)
}

/// Ask Aurora for the bindings once (in the background).
fn load() {
    if !crate::pads::steam_input_enabled() {
        let mut s = st();
        if !s.disabled_logged {
            s.disabled_logged = true;
            crate::log!("input: Steam Input disabled for this game");
        }
        return;
    }
    {
        let mut s = st();
        if s.bindings.is_some() || s.loading {
            return;
        }
        s.loading = true;
    }
    std::thread::spawn(|| {
        let (text, source) = match crate::ipc::request(Request::InputConfig) {
            Some(Reply::InputConfig { bindings, source }) => (bindings, source),
            _ => (String::new(), String::new()),
        };
        let parsed = Bindings::parse(&text);
        match &parsed {
            Some(b) => crate::log!("input: bindings from {source}: {} group(s), {} set(s)/layer(s)", b.groups.len(), b.presets.len()),
            None => crate::log!("input: no Steam Input bindings for a pad; the game keeps its own controller support"),
        }
        let mut s = st();
        s.loading = false;
        s.bindings = Some(parsed.unwrap_or_default());
    });
}

fn has_bindings(s: &Input) -> bool {
    s.bindings.as_ref().is_some_and(|b| !b.presets.is_empty())
}

/// Read the pads (pads.rs asks empty XInput slots only once a second).
fn poll(s: &mut Input) {
    if !crate::pads::steam_input_enabled() {
        s.pads = [None; 4];
        return;
    }
    if s.polled.is_some_and(|t| t.elapsed() < Duration::from_millis(2)) {
        return;
    }
    s.polled = Some(Instant::now());
    s.pads = crate::pads::steam_input_pads();
}

fn intern(list: &mut Vec<String>, name: &str) -> u64 {
    match list.iter().position(|n| n == name) {
        Some(i) => i as u64 + 1,
        None => {
            list.push(name.to_string());
            list.len() as u64
        }
    }
}

const ALL: u64 = u64::MAX;

/// The pad, the active set's name, active layers, and calibration context for a controller.
fn context(s: &mut Input, handle: u64) -> Option<(Pad, bool, PadCalibration, String, Vec<String>)> {
    poll(s);
    let mut entry = s.pads.get(handle.checked_sub(1)? as usize).copied().flatten()?;
    let c = cal(s);
    if !entry.calibrated && c.nintendo_layout {
        entry.pad.buttons = pads_shm::swap_nintendo_buttons(entry.pad.buttons);
    }
    let (set, layers) = s.active.get(&handle).or_else(|| s.active.get(&ALL)).cloned().unwrap_or((1, Vec::new()));
    let name = |h: u64| s.sets.get(h.saturating_sub(1) as usize).cloned().unwrap_or_default();
    Some((entry.pad, entry.calibrated, c, name(set), layers.into_iter().map(name).collect()))
}

// ----------------------------------------------------------------- exports

extern "C" fn init(_t: This) -> bool {
    load();
    true
}

extern "C" fn run_frame(_t: This) {
    poll(&mut st());
}

extern "C" fn connected(_t: This, out: *mut u64) -> i32 {
    if !crate::pads::steam_input_enabled() {
        return 0;
    }
    load();
    let mut s = st();
    if !has_bindings(&s) {
        return 0;
    }
    poll(&mut s);
    let mut n = 0;
    for (i, p) in s.pads.iter().enumerate() {
        if p.is_some() {
            if !out.is_null() {
                unsafe { out.add(n).write_unaligned(i as u64 + 1) };
            }
            n += 1;
        }
    }
    n as i32
}

extern "C" fn set_handle(_t: This, name: *const c_char) -> u64 {
    let n = unsafe { text(name) };
    intern(&mut st().sets, &n)
}

extern "C" fn activate_set(_t: This, handle: u64, set: u64) {
    let mut s = st();
    if handle == ALL {
        s.active.retain(|_, (cur, _)| {
            *cur = set;
            true
        });
    }
    s.active.entry(handle).or_default().0 = set;
}

extern "C" fn current_set(_t: This, handle: u64) -> u64 {
    st().active.get(&handle).map(|a| a.0).unwrap_or(0)
}

extern "C" fn activate_layer(_t: This, handle: u64, layer: u64) {
    let mut s = st();
    let layers = &mut s.active.entry(handle).or_insert((1, Vec::new())).1;
    if !layers.contains(&layer) {
        layers.push(layer);
    }
}

extern "C" fn deactivate_layer(_t: This, handle: u64, layer: u64) {
    if let Some(a) = st().active.get_mut(&handle) {
        a.1.retain(|l| *l != layer);
    }
}

extern "C" fn deactivate_layers(_t: This, handle: u64) {
    if let Some(a) = st().active.get_mut(&handle) {
        a.1.clear();
    }
}

extern "C" fn active_layers(_t: This, handle: u64, out: *mut u64) -> i32 {
    let s = st();
    let layers = s.active.get(&handle).map(|a| a.1.clone()).unwrap_or_default();
    for (i, l) in layers.iter().enumerate() {
        if !out.is_null() {
            unsafe { out.add(i).write_unaligned(*l) };
        }
    }
    layers.len() as i32
}

extern "C" fn digital_handle(_t: This, name: *const c_char) -> u64 {
    let n = unsafe { text(name) };
    intern(&mut st().actions, &n)
}

extern "C" fn analog_handle(_t: This, name: *const c_char) -> u64 {
    let n = unsafe { text(name) };
    intern(&mut st().actions, &n)
}

/// InputDigitalActionData_t: bState, bActive.
extern "C" fn digital_data(_t: This, ret: *mut u8, handle: u64, action: u64) -> *mut u8 {
    let mut s = st();
    let name = s.actions.get(action.saturating_sub(1) as usize).cloned().unwrap_or_default();
    let (mut state, mut active) = (false, false);
    if let Some((pad, calibrated, cal, set, layers)) = context(&mut s, handle) {
        let edge = if calibrated { 0 } else { (cal.trigger_threshold * 255.0).round().max(1.0) as u8 };
        if let Some(b) = s.bindings.as_ref() {
            for (source, g) in b.active(&set, &layers) {
                for (input, acts) in &g.inputs {
                    if acts.contains(&name) {
                        active = true;
                        state |= pressed(&pad, source, &g.mode, input, edge);
                    }
                }
            }
        }
    }
    if !ret.is_null() {
        unsafe { ret.copy_from_nonoverlapping([state as u8, active as u8].as_ptr(), 2) };
    }
    ret
}

/// InputAnalogActionData_t: eMode, x, y, bActive.
extern "C" fn analog_data(_t: This, ret: *mut u8, handle: u64, action: u64) -> *mut u8 {
    let mut s = st();
    let name = s.actions.get(action.saturating_sub(1) as usize).cloned().unwrap_or_default();
    let mut b = Bytes::new(16);
    if let Some((pad, calibrated, cal, set, layers)) = context(&mut s, handle) {
        if let Some(bind) = s.bindings.as_ref() {
            // The strongest source wins when several drive the action.
            let best = bind
                .active(&set, &layers)
                .into_iter()
                .filter(|(_, g)| g.analog.contains(&name))
                .map(|(src, g)| analog_value(&pad, src, &g.mode, &cal, calibrated))
                .max_by(|a, b| (a.1.abs() + a.2.abs()).total_cmp(&(b.1.abs() + b.2.abs())));
            if let Some((mode, x, y)) = best {
                b.i32(0, mode).u32(4, x.to_bits()).u32(8, y.to_bits()).u8(12, 1);
            }
        }
    }
    if !ret.is_null() {
        unsafe { ret.copy_from_nonoverlapping(b.0.as_ptr(), 16) };
    }
    ret
}

/// Origins bound to an action in a set (at most 8, like k_unMaxOrigins).
fn origins(set: u64, action: u64, analog: bool, out: *mut i32) -> i32 {
    let s = st();
    let (Some(b), Some(set), Some(name)) = (s.bindings.as_ref(), s.sets.get(set.saturating_sub(1) as usize), s.actions.get(action.saturating_sub(1) as usize)) else { return 0 };
    let mut list = Vec::new();
    for (source, g) in b.active(set, &[]) {
        if analog && g.analog.contains(name) {
            list.push(origin(source, ""));
        }
        if !analog {
            list.extend(g.inputs.iter().filter(|(_, a)| a.contains(name)).map(|(i, _)| origin(source, i)));
        }
    }
    list.retain(|o| *o != 0);
    list.dedup();
    list.truncate(8);
    for (i, o) in list.iter().enumerate() {
        if !out.is_null() {
            unsafe { out.add(i).write_unaligned(*o) };
        }
    }
    list.len() as i32
}

extern "C" fn digital_origins(_t: This, _handle: u64, set: u64, action: u64, out: *mut i32) -> i32 {
    origins(set, action, false, out)
}

extern "C" fn analog_origins(_t: This, _handle: u64, set: u64, action: u64, out: *mut i32) -> i32 {
    origins(set, action, true, out)
}

extern "C" fn action_name(_t: This, action: u64) -> *const c_char {
    let n = st().actions.get(action.saturating_sub(1) as usize).cloned().unwrap_or_default();
    lock().intern(&n)
}

extern "C" fn origin_name(_t: This, o: i32) -> *const c_char {
    lock().intern(origin_text(o))
}

extern "C" fn vibrate(_t: This, handle: u64, left: u16, right: u16) {
    let slots: Vec<usize> = if handle == ALL { (0..4).collect() } else { vec![handle.saturating_sub(1) as usize] };
    for i in slots {
        crate::pads::rumble(i, left, right);
    }
}

/// InputMotionData_t (40 bytes): rotQuat (xyzw), posAccel (xyz in G), rotVel (xyz in deg/s).
extern "C" fn motion_data(_t: This, ret: *mut u8, handle: u64) -> *mut u8 {
    if !ret.is_null() {
        let mut b = Bytes::new(40);
        // Identity orientation quaternion (0, 0, 0, 1).
        b.u32(12, 1.0f32.to_bits());
        if let Some(slot) = handle.checked_sub(1).filter(|i| *i < 4)
            && let Some((accel, gyro)) = crate::pads::motion(slot as usize)
        {
            const G: f32 = 9.80665;
            b.u32(16, (accel[0] / G).to_bits());
            b.u32(20, (accel[1] / G).to_bits());
            b.u32(24, (accel[2] / G).to_bits());
            b.u32(28, gyro[0].to_degrees().to_bits());
            b.u32(32, gyro[1].to_degrees().to_bits());
            b.u32(36, gyro[2].to_degrees().to_bits());
        }
        unsafe { std::ptr::copy_nonoverlapping(b.0.as_ptr(), ret, 40) };
    }
    ret
}

/// ESteamInputType: 3 = Xbox One controller.
extern "C" fn input_type(_t: This, handle: u64) -> i32 {
    if !crate::pads::steam_input_enabled() {
        return 0;
    }
    let mut s = st();
    poll(&mut s);
    if s.pads.get(handle.saturating_sub(1) as usize).copied().flatten().is_some() { 3 } else { 0 }
}

extern "C" fn for_index(_t: This, i: i32) -> u64 {
    if (0..4).contains(&i) { i as u64 + 1 } else { 0 }
}

extern "C" fn index_of(_t: This, handle: u64) -> i32 {
    if (1..=4).contains(&handle) { handle as i32 - 1 } else { -1 }
}

/// EXboxOrigin → EInputActionOrigin (A, B, X, Y, LB, RB, Menu, View, LT pull/click, RT…).
extern "C" fn from_xbox_origin(_t: This, _handle: u64, x: i32) -> i32 {
    const MAP: [i32; 33] = [114, 115, 116, 117, 118, 119, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 0, 0, 0, 0, 0];
    MAP.get(x.max(0) as usize).copied().unwrap_or(0)
}

extern "C" fn translate_origin(_t: This, _kind: i32, o: i32) -> i32 {
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r##""controller_mappings"
{
	"version"		"3"
	"controller_type"		"controller_xboxone"
	"actions" { "InGame" { "title" "#Set" } "Menu" { "title" "#Menu" } }
	"action_layers" { "Aim" { "title" "#Aim" } }
	"group" { "id" "0" "mode" "four_buttons" "inputs" { "button_a" { "activators" { "Full_Press" { "bindings" { "binding" "game_action InGame jump, Jump, " } } } } } }
	"group" { "id" "1" "mode" "joystick_move" "gameactions" { "InGame" "Move" } }
	"group" { "id" "2" "mode" "trigger" "inputs" { "click" { "activators" { "Full_Press" { "bindings" { "binding" "game_action InGame fire, , " } } } } } "gameactions" { "InGame" "Throttle" } }
	"group" { "id" "3" "mode" "four_buttons" "inputs" { "button_a" { "activators" { "Full_Press" { "bindings" { "binding" "game_action Menu select, , " } } } } } }
	"preset" { "id" "0" "name" "InGame" "group_source_bindings" { "0" "button_diamond active" "1" "left_joystick active" "2" "right_trigger active" } }
	"preset" { "id" "1" "name" "Menu" "group_source_bindings" { "3" "button_diamond active" } }
	"preset" { "id" "2" "name" "Aim" "group_source_bindings" { "3" "button_diamond active" } }
}"##;

    #[test]
    fn maps_pad_to_actions() {
        let b = Bindings::parse(CONFIG).unwrap();
        assert_eq!(b.layers, vec!["Aim"]);
        let pad = Pad { buttons: A, rt: 255, lx: 32767, ..Default::default() };
        let cal = PadCalibration::default();
        let act = b.active("InGame", &[]);
        let jump = act.iter().any(|(s, g)| g.inputs.iter().any(|(i, a)| a.contains(&"jump".to_string()) && pressed(&pad, s, &g.mode, i, 30)));
        let fire = act.iter().any(|(s, g)| g.inputs.iter().any(|(i, a)| a.contains(&"fire".to_string()) && pressed(&pad, s, &g.mode, i, 30)));
        assert!(jump && fire);
        let (mode, x, _) = act.iter().find(|(_, g)| g.analog.contains(&"Move".to_string())).map(|(s, g)| analog_value(&pad, s, &g.mode, &cal, false)).unwrap();
        assert_eq!(mode, 6);
        assert!(x > 0.99);
        // Deadzone filters small stick movement on uncalibrated pads, skips double-deadzone on calibrated pads.
        let small = Pad { lx: 4000, ..Default::default() };
        assert_eq!(analog_value(&small, "left_joystick", "joystick_move", &cal, false).1, 0.0);
        assert!(analog_value(&small, "left_joystick", "joystick_move", &cal, true).1 > 0.1);
        // The layer replaces button_diamond: A now selects.
        let layered = b.active("InGame", &["Aim".to_string()]);
        assert!(layered.iter().any(|(s, g)| *s == "button_diamond" && g.inputs.iter().any(|(_, a)| a.contains(&"select".to_string()))));
        assert_eq!(origin("right_trigger", "click"), 129);
    }
}
