//! Ordered Steam controller documents and the legacy bindings shared by the
//! desktop editor and the game bridge.
use crate::{Obj, Value};
use std::collections::HashMap;

pub const MAX_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SETS: usize = 64;
pub const MAX_BINDINGS_PER_SET: usize = 256;
pub const MAX_BINDINGS: usize = 2048;
pub const MAX_ACTIONS: usize = 4096;
pub const MAX_ACTIONS_PER_INPUT: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error(transparent)]
    Vdf(#[from] crate::Error),
    #[error("missing controller_mappings object")]
    MissingMappings,
    #[error("controller layout exceeds its size or binding limits")]
    Limit,
    #[error("invalid Aurora controller binding")]
    InvalidBinding,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutDocument(Obj);

#[derive(Default, Clone, Copy)]
struct Count {
    bindings: usize,
    actions: usize,
}

impl Count {
    fn add(&mut self, rhs: Self) {
        self.bindings += rhs.bindings;
        self.actions += rhs.actions;
    }

    fn check(self, max_bindings: usize) -> Result<(), LayoutError> {
        if self.bindings > max_bindings || self.actions > MAX_ACTIONS {
            return Err(LayoutError::Limit);
        }
        Ok(())
    }
}

/// Steam stores binding commands in ordered activators with duplicate keys.
pub fn commands(input: &Obj) -> impl Iterator<Item = &str> {
    input.obj("activators").into_iter().flat_map(Obj::iter).filter_map(|(_, act)| {
        act.as_obj()?.obj("bindings")
    }).flat_map(Obj::iter).filter_map(|(key, value)| {
        key.eq_ignore_ascii_case("binding").then(|| value.as_str()).flatten()
    })
}

impl LayoutDocument {
    pub fn parse(text: &str) -> Result<Self, LayoutError> {
        if text.len() > MAX_BYTES {
            return Err(LayoutError::Limit);
        }
        let root = crate::parse(text.strip_prefix('\u{feff}').unwrap_or(text))?;
        let mappings = root.obj("controller_mappings").ok_or(LayoutError::MissingMappings)?;
        let doc = Self(root.clone());
        doc.validate(mappings)?;
        // The canonical encoding must fit the same on-disk limit.
        if doc.write().len() > MAX_BYTES {
            return Err(LayoutError::Limit);
        }
        Ok(Self(root))
    }

    pub fn automatic() -> Self {
        Self(Obj::default().with_obj("controller_mappings", Obj::default()
            .with("version", "3").with("controller_type", "controller_xboxone")))
    }

    pub fn write(&self) -> String {
        crate::write(&self.0)
    }

    pub fn mappings(&self) -> &Obj {
        self.0.obj("controller_mappings").expect("validated layout root")
    }

    pub fn title(&self) -> Option<&str> {
        let raw = self.mappings().str("title").filter(|title| !title.is_empty())?;
        if let Some(token) = raw.strip_prefix('#') {
            if let Some(loc) = self.mappings().obj("localization") {
                if let Some(val) = loc
                    .obj("english")
                    .and_then(|lang| lang.str(token))
                    .filter(|s| !s.is_empty())
                {
                    return Some(val);
                }
                for (_, lang_val) in loc.iter() {
                    if let Some(val) = lang_val
                        .as_obj()
                        .and_then(|lang| lang.str(token))
                        .filter(|s| !s.is_empty())
                    {
                        return Some(val);
                    }
                }
            }
            return None;
        }
        Some(raw)
    }

    pub fn set_title(&mut self, title: &str) {
        let (_, Value::Obj(mappings)) = self
            .0
            .0
            .iter_mut()
            .find(|(k, v)| k.eq_ignore_ascii_case("controller_mappings") && v.as_obj().is_some())
            .unwrap()
        else {
            unreachable!()
        };
        let clean: String = title.chars().filter(|c| !c.is_control()).take(128).collect();
        mappings.0.retain(|(key, _)| !key.eq_ignore_ascii_case("title"));
        if !clean.trim().is_empty() {
            mappings.0.push(("title".into(), Value::Str(clean.trim().to_string())));
        }
    }

    pub fn clear_binding(&mut self, input: Control) {
        let (_, Value::Obj(mappings)) = self
            .0
            .0
            .iter_mut()
            .find(|(k, v)| k.eq_ignore_ascii_case("controller_mappings") && v.as_obj().is_some())
            .unwrap()
        else {
            unreachable!()
        };
        if let Some((_, Value::Obj(bindings))) = mappings.0.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case("aurora_bindings")) {
            bindings.0.retain(|(key, _)| !key.eq_ignore_ascii_case(input.name()));
        }
    }

    /// Collect all distinct `(action_set, action_name)` pairs defined or
    /// referenced by a native Steam Input document.
    pub fn native_actions(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        let m = self.mappings();
        for (_, value) in m.iter().filter(|(k, _)| k.eq_ignore_ascii_case("group")) {
            let Some(group) = value.as_obj() else { continue };
            for (_, raw) in group.obj("inputs").into_iter().flat_map(Obj::iter) {
                let Some(inp) = raw.as_obj() else { continue };
                for cmd in commands(inp) {
                    if let Some(Output::Native(set, act)) = Output::parse(cmd) {
                        if !out.iter().any(|(s, a)| s == &set && a == &act) {
                            out.push((set, act));
                        }
                    }
                }
            }
            if let Some(ga) = group.obj("gameactions") {
                for (set, act) in ga.iter() {
                    if let Some(act_name) = act.as_str() {
                        if !out.iter().any(|(s, a)| s == set && a == act_name) {
                            out.push((set.to_string(), act_name.to_string()));
                        }
                    }
                }
            }
        }
        out
    }

    fn validate(&self, m: &Obj) -> Result<(), LayoutError> {
        let mut groups = HashMap::new();
        let mut definitions = Count::default();
        let mut sets = 0;
        let mut references = 0;
        let mut expanded = Count::default();
        for (_, value) in m.iter().filter(|(k, _)| k.eq_ignore_ascii_case("group")) {
            let Some(group) = value.as_obj() else { continue };
            let mut count = Count::default();
            for (_, input) in group.obj("inputs").into_iter().flat_map(Obj::iter) {
                let actions = input.as_obj().map_or(0, |input| commands(input).count());
                if actions > MAX_ACTIONS_PER_INPUT {
                    return Err(LayoutError::Limit);
                }
                count.add(Count { bindings: 1, actions });
            }
            if let Some(analog) = group.obj("gameactions") {
                if analog.0.len() > MAX_ACTIONS_PER_INPUT {
                    return Err(LayoutError::Limit);
                }
                count.add(Count { bindings: 1, actions: analog.0.len() });
            }
            definitions.add(count);
            definitions.check(MAX_BINDINGS)?;
            if let Some(id) = group.str("id") {
                groups.insert(id, count);
            }
        }
        for (_, value) in m.iter().filter(|(k, _)| k.eq_ignore_ascii_case("preset")) {
            sets += 1;
            let mut count = Count::default();
            let mut seen = std::collections::HashSet::new();
            for (id, source) in value.as_obj().and_then(|p| p.obj("group_source_bindings")).into_iter().flat_map(Obj::iter) {
                references += 1;
                if references > MAX_BINDINGS {
                    return Err(LayoutError::Limit);
                }
                let mut words = source.as_str().unwrap_or_default().split_whitespace();
                if words.next().is_some() && words.next() == Some("active") {
                    if !seen.insert(id) {
                        return Err(LayoutError::InvalidBinding);
                    }
                    if let Some(group) = groups.get(id) {
                        count.add(*group);
                    }
                }
            }
            count.check(MAX_BINDINGS_PER_SET)?;
            expanded.add(count);
        }
        for key in ["actions", "action_layers"] {
            if m.obj(key).is_some_and(|sets| sets.0.len() > MAX_SETS) {
                return Err(LayoutError::Limit);
            }
        }
        let overrides = self.overrides().count();
        if sets > MAX_SETS || overrides > MAX_BINDINGS_PER_SET {
            return Err(LayoutError::Limit);
        }
        let overrides = Count { bindings: overrides, actions: overrides };
        definitions.add(overrides);
        expanded.add(overrides);
        definitions.check(MAX_BINDINGS)?;
        expanded.check(MAX_BINDINGS)
    }

    pub fn overrides(&self) -> impl Iterator<Item = (Control, Output)> + '_ {
        self.mappings().obj("aurora_bindings").into_iter().flat_map(Obj::iter).filter_map(|(name, value)| {
            Some((Control::named(name)?, Output::parse(value.as_str()?)?))
        })
    }

    pub fn set_binding(&mut self, input: Control, output: &Output) {
        let (_, Value::Obj(mappings)) = self.0.0.iter_mut().find(|(k, v)| k.eq_ignore_ascii_case("controller_mappings") && v.as_obj().is_some()).unwrap() else {
            unreachable!()
        };
        if mappings.obj("aurora_bindings").is_none() {
            mappings.0.retain(|(key, _)| !key.eq_ignore_ascii_case("aurora_bindings"));
            mappings.0.push(("aurora_bindings".into(), Value::Obj(Obj::default())));
        }
        let (_, Value::Obj(bindings)) = mappings.0.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case("aurora_bindings")).unwrap() else {
            unreachable!()
        };
        bindings.0.retain(|(key, _)| !key.eq_ignore_ascii_case(input.name()));
        bindings.0.push((input.name().into(), Value::Str(output.encode())));
    }

    /// Compile Steam's simple legacy groups from the default preset. Aurora
    /// overrides apply last. Native Steam Input presets use their own actions.
    pub fn legacy_map(&self) -> LegacyMap {
        let mut map = LegacyMap::default();
        let m = self.mappings();
        let groups: HashMap<_, _> = m.iter().filter(|(k, _)| k.eq_ignore_ascii_case("group"))
            .filter_map(|(_, v)| { let group = v.as_obj()?; Some((group.str("id")?, group)) }).collect();
        let presets = || m.iter().filter(|(k, _)| k.eq_ignore_ascii_case("preset")).filter_map(|(_, v)| v.as_obj());
        let preset = presets().find(|p| p.str("id") == Some("0")).or_else(|| presets().next());
        for (id, raw) in preset.and_then(|p| p.obj("group_source_bindings")).into_iter().flat_map(Obj::iter) {
            let mut words = raw.as_str().unwrap_or_default().split_whitespace();
            let Some(source) = words.next() else { continue };
            if words.next() != Some("active") {
                continue;
            }
            let Some(group) = groups.get(id) else { continue };
            if let Some(control) = Control::from_steam(source, "") {
                let output = match group.str("mode") {
                    Some("absolute_mouse" | "relative_mouse" | "mouse_joystick") => Some(Output::MouseMove),
                    Some("joystick_move") if Control::stick(source).is_some() && group.obj("gameactions").is_none() => {
                        let swap = group.path(&["settings"]).and_then(|s| s.str("output_joystick")) == Some("1");
                        Some(Output::Pad(if swap { Control::RightStick } else { Control::LeftStick }))
                    }
                    _ => None,
                };
                if let Some(output) = output {
                    map.0.insert(control, vec![output]);
                }
            }
            for (name, raw) in group.obj("inputs").into_iter().flat_map(Obj::iter) {
                let Some(control) = Control::from_steam(source, name) else { continue };
                let Some(input) = raw.as_obj() else { continue };
                let outputs: Vec<_> = commands(input).filter_map(Output::parse)
                    .filter(|o| !matches!(o, Output::Native(..))).collect();
                if !outputs.is_empty() {
                    map.0.insert(control, outputs);
                }
            }
        }
        for (input, output) in self.overrides() {
            map.0.insert(input, vec![output]);
        }
        map
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyMap(pub std::collections::BTreeMap<Control, Vec<Output>>);

macro_rules! controls {
    ($($id:ident => ($name:literal, $source:literal, $input:literal)),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        pub enum Control { $($id),+ }
        impl Control {
            pub const ALL: &[Self] = &[$(Self::$id),+];
            pub fn name(self) -> &'static str {
                match self { $(Self::$id => $name),+ }
            }
            pub fn from_steam(source: &str, input: &str) -> Option<Self> {
                match (source, input) { $(($source, $input) => Some(Self::$id),)+ _ => None }
            }
            pub fn steam(self) -> (&'static str, &'static str) {
                match self { $(Self::$id => ($source, $input)),+ }
            }
        }
    };
}

controls! {
    A => ("A", "button_diamond", "button_a"),
    B => ("B", "button_diamond", "button_b"),
    X => ("X", "button_diamond", "button_x"),
    Y => ("Y", "button_diamond", "button_y"),
    LB => ("LB", "switch", "left_bumper"),
    RB => ("RB", "switch", "right_bumper"),
    LT => ("LT", "left_trigger", "edge"),
    RT => ("RT", "right_trigger", "edge"),
    L3 => ("L3", "left_joystick", "click"),
    R3 => ("R3", "right_joystick", "click"),
    Start => ("Start", "switch", "button_escape"),
    Back => ("Back", "switch", "button_menu"),
    Up => ("D-pad ↑", "dpad", "dpad_north"),
    Down => ("D-pad ↓", "dpad", "dpad_south"),
    Left => ("D-pad ←", "dpad", "dpad_west"),
    Right => ("D-pad →", "dpad", "dpad_east"),
    L4 => ("L4", "switch", "button_back_left"),
    R4 => ("R4", "switch", "button_back_right"),
    L5 => ("L5", "switch", "button_back_left2"),
    R5 => ("R5", "switch", "button_back_right2"),
    LeftStick => ("Left stick", "left_joystick", ""),
    RightStick => ("Right stick", "right_joystick", ""),
    LeftStickUp => ("LS ↑", "left_joystick", "dpad_north"),
    LeftStickDown => ("LS ↓", "left_joystick", "dpad_south"),
    LeftStickLeft => ("LS ←", "left_joystick", "dpad_west"),
    LeftStickRight => ("LS →", "left_joystick", "dpad_east"),
    RightStickUp => ("RS ↑", "right_joystick", "dpad_north"),
    RightStickDown => ("RS ↓", "right_joystick", "dpad_south"),
    RightStickLeft => ("RS ←", "right_joystick", "dpad_west"),
    RightStickRight => ("RS →", "right_joystick", "dpad_east"),
    Gyro => ("Gyroscope", "gyro", ""),
}

impl Control {
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.name().eq_ignore_ascii_case(name)).or_else(|| match name.to_ascii_uppercase().as_str() {
            "LEFT_BUMPER" => Some(Self::LB), "RIGHT_BUMPER" => Some(Self::RB),
            "LEFT_TRIGGER" => Some(Self::LT), "RIGHT_TRIGGER" => Some(Self::RT),
            "LEFT_STICK" => Some(Self::L3), "RIGHT_STICK" => Some(Self::R3),
            "DPAD_UP" => Some(Self::Up), "DPAD_DOWN" => Some(Self::Down),
            "DPAD_LEFT" => Some(Self::Left), "DPAD_RIGHT" => Some(Self::Right),
            _ => None,
        })
    }

    fn stick(source: &str) -> Option<Self> {
        Self::from_steam(source, "").filter(|c| matches!(c, Self::LeftStick | Self::RightStick))
    }

    pub fn button(self) -> u16 {
        match self {
            Self::A => 0x1000, Self::B => 0x2000, Self::X => 0x4000, Self::Y => 0x8000,
            Self::LB => 0x0100, Self::RB => 0x0200, Self::L3 => 0x0040, Self::R3 => 0x0080,
            Self::Start => 0x0010, Self::Back => 0x0020, Self::Up => 1, Self::Down => 2,
            Self::Left => 4, Self::Right => 8, _ => 0,
        }
    }

    pub fn pad_output(self) -> bool {
        self.button() != 0 || matches!(self, Self::LT | Self::RT | Self::LeftStick | Self::RightStick)
    }

    pub fn vector(self) -> bool {
        matches!(self, Self::LeftStick | Self::RightStick | Self::Gyro)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Output {
    Unassigned,
    Pad(Control),
    Key(u16),
    MouseButton(u8),
    Wheel(i8),
    MouseMove,
    Native(String, String),
}

impl Output {
    pub fn parse(command: &str) -> Option<Self> {
        let raw = command.split(',').next()?.trim();
        let mut words = raw.splitn(2, char::is_whitespace);
        let kind = words.next()?;
        let value = words.next().unwrap_or_default().trim();
        match kind.to_ascii_lowercase().as_str() {
            "unassigned" if value.is_empty() => Some(Self::Unassigned),
            "pad" | "xinput_button" | "gamepad_button" => {
                let control = Control::named(value)?;
                control.pad_output().then_some(Self::Pad(control))
            }
            "key_press" => key_code(value).map(Self::Key),
            "mouse_button" => match value.to_ascii_uppercase().as_str() {
                "LEFT" | "1" => Some(Self::MouseButton(1)),
                "RIGHT" | "2" => Some(Self::MouseButton(2)),
                "MIDDLE" | "3" => Some(Self::MouseButton(3)),
                "4" => Some(Self::MouseButton(4)),
                "5" => Some(Self::MouseButton(5)),
                "WHEEL_UP" => Some(Self::Wheel(1)),
                "WHEEL_DOWN" => Some(Self::Wheel(-1)),
                _ => None,
            },
            "mouse_move" if value.is_empty() => Some(Self::MouseMove),
            "game_action" => {
                let mut names = value.split_whitespace();
                Some(Self::Native(names.next()?.into(), names.next()?.into()))
            }
            _ => None,
        }
    }

    pub fn encode(&self) -> String {
        match self {
            Self::Unassigned => "unassigned".into(),
            Self::Pad(c) => format!("pad {}", c.name()),
            Self::Key(k) => format!("key_press VK_{k:02X}"),
            Self::MouseButton(b) => format!("mouse_button {b}"),
            Self::Wheel(n) => format!("mouse_button WHEEL_{}", if *n > 0 { "UP" } else { "DOWN" }),
            Self::MouseMove => "mouse_move".into(),
            Self::Native(set, action) => format!("game_action {set} {action}"),
        }
    }

    pub fn display_label(&self) -> String {
        match self {
            Self::Unassigned => "Unassigned".into(),
            Self::Pad(c) => format!("Pad: {}", c.name()),
            Self::Key(k) => format!("Key: {}", key_name(*k)),
            Self::MouseButton(1) => "Mouse: Left".into(),
            Self::MouseButton(2) => "Mouse: Right".into(),
            Self::MouseButton(3) => "Mouse: Middle".into(),
            Self::MouseButton(n) => format!("Mouse: Button {n}"),
            Self::Wheel(n) if *n > 0 => "Mouse: Wheel Up".into(),
            Self::Wheel(_) => "Mouse: Wheel Down".into(),
            Self::MouseMove => "Relative Mouse".into(),
            Self::Native(set, action) => format!("{set} · {action}"),
        }
    }
}

pub fn key_name(vk: u16) -> String {
    match vk {
        0x08 => "Backspace".into(),
        0x09 => "Tab".into(),
        0x0d => "Enter".into(),
        0x10 => "Shift".into(),
        0x11 => "Ctrl".into(),
        0x12 => "Alt".into(),
        0x1b => "Esc".into(),
        0x20 => "Space".into(),
        0x21 => "Page Up".into(),
        0x22 => "Page Down".into(),
        0x23 => "End".into(),
        0x24 => "Home".into(),
        0x25 => "Left".into(),
        0x26 => "Up".into(),
        0x27 => "Right".into(),
        0x28 => "Down".into(),
        0x2d => "Insert".into(),
        0x2e => "Delete".into(),
        0x30..=0x39 | 0x41..=0x5a => (vk as u8 as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6f),
        0xba => ";".into(),
        0xbb => "=".into(),
        0xbc => ",".into(),
        0xbd => "-".into(),
        0xbe => ".".into(),
        0xbf => "/".into(),
        0xc0 => "`".into(),
        0xdb => "[".into(),
        0xdc => "\\".into(),
        0xdd => "]".into(),
        0xde => "'".into(),
        _ => format!("VK_{vk:02X}"),
    }
}

pub fn key_code(key: &str) -> Option<u16> {
    let uppercase = key.to_ascii_uppercase();
    let name = uppercase.trim_start_matches("KEY_");
    let code = match name.as_bytes() {
        [c] if c.is_ascii_alphanumeric() => u16::from(*c),
        _ => match name {
            "BACKSPACE" => 0x08, "TAB" => 0x09, "ENTER" | "RETURN" => 0x0d,
            "SHIFT" | "LSHIFT" => 0x10, "CTRL" | "LCTRL" | "CONTROL" => 0x11,
            "ALT" | "LALT" => 0x12, "ESC" | "ESCAPE" => 0x1b, "SPACE" => 0x20,
            "PAGEUP" | "PGUP" => 0x21, "PAGEDOWN" | "PGDN" => 0x22,
            "END" => 0x23, "HOME" => 0x24,
            "LEFT" => 0x25, "UP" => 0x26, "RIGHT" => 0x27, "DOWN" => 0x28,
            "INSERT" => 0x2d, "DELETE" => 0x2e,
            "SEMICOLON" => 0xba, "EQUAL" | "EQUALS" => 0xbb, "COMMA" => 0xbc,
            "MINUS" => 0xbd, "PERIOD" => 0xbe, "SLASH" => 0xbf, "BACKQUOTE" => 0xc0,
            "LEFTBRACKET" => 0xdb, "BACKSLASH" => 0xdc, "RIGHTBRACKET" => 0xdd,
            "APOSTROPHE" => 0xde,
            _ if name.starts_with("F") => 0x6f + name[1..].parse::<u16>().ok().filter(|f| (1..=24).contains(f))?,
            _ => u16::from_str_radix(name.strip_prefix("VK_")?, 16).ok()?,
        },
    };
    (matches!(code, 0x08 | 0x09 | 0x0d | 0x10..=0x12 | 0x1b | 0x20..=0x28 | 0x2d..=0x2e | 0x30..=0x39 | 0x41..=0x5a | 0x60..=0x87 | 0xba..=0xc0 | 0xdb..=0xde)).then_some(code)
}
