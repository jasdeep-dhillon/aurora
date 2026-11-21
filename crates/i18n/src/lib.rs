//! Interface translations.
//!
//! The English text *is* the key, so code stays readable:
//!
//! ```ignore
//! ui.label(t!("Sign in"));
//! ui.label(t!("Playing {game}", game = name));
//! ```
//!
//! Other languages live in `locales/<code>.toml` as `"English" = "Translation"`.
//! Missing entries fall back to English. To add a language, add a file and a
//! [`Lang`] variant. Tests check that every `t!` string in the app has an
//! entry in every language, with the same `{placeholders}`.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    English,
    Russian,
    French,
    Spanish,
}

impl Lang {
    pub const ALL: [Lang; 4] = [Lang::English, Lang::French, Lang::Spanish, Lang::Russian];

    pub fn code(self) -> &'static str {
        match self {
            Lang::English => "en",
            Lang::Russian => "ru",
            Lang::French => "fr",
            Lang::Spanish => "es",
        }
    }
    /// The language's own name.
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::English => "English",
            Lang::Russian => "Русский",
            Lang::French => "Français",
            Lang::Spanish => "Español",
        }
    }
    /// Name Steam's store API expects (`l=` parameter).
    pub fn steam_name(self) -> &'static str {
        match self {
            Lang::English => "english",
            Lang::Russian => "russian",
            Lang::French => "french",
            Lang::Spanish => "spanish",
        }
    }
    pub fn from_code(code: &str) -> Option<Lang> {
        Lang::ALL.into_iter().find(|l| code.to_lowercase().starts_with(l.code()))
    }
    /// The system language if we have it, otherwise English.
    pub fn detect() -> Lang {
        sys_locale::get_locale().and_then(|l| Lang::from_code(&l)).unwrap_or(Lang::English)
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_language(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn language() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::Russian,
        2 => Lang::French,
        3 => Lang::Spanish,
        _ => Lang::English,
    }
}

fn table(lang: Lang) -> Option<&'static HashMap<String, &'static str>> {
    static RU: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    static FR: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    static ES: OnceLock<HashMap<String, &'static str>> = OnceLock::new();
    match lang {
        Lang::English => None,
        Lang::Russian => Some(RU.get_or_init(|| load(include_str!("../locales/ru.toml")))),
        Lang::French => Some(FR.get_or_init(|| load(include_str!("../locales/fr.toml")))),
        Lang::Spanish => Some(ES.get_or_init(|| load(include_str!("../locales/es.toml")))),
    }
}

fn load(src: &str) -> HashMap<String, &'static str> {
    let parsed: toml::Table = toml::from_str(src).expect("valid locale file");
    parsed
        .into_iter()
        .filter_map(|(k, v)| Some((k, &*Box::leak(v.as_str()?.to_string().into_boxed_str()))))
        .collect()
}

/// The name the interface shows for the app.
pub const BRAND: &str = "Aurora";

/// "SteamInRust" in interface text becomes [`BRAND`]. Each changed string is
/// built once and kept (there are only so many labels).
fn rebrand(s: &'static str) -> &'static str {
    if !s.contains("SteamInRust") {
        return s;
    }
    static CACHE: OnceLock<std::sync::Mutex<HashMap<&'static str, &'static str>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    cache.entry(s).or_insert_with(|| Box::leak(s.replace("SteamInRust", BRAND).into_boxed_str()))
}

fn strip_context(s: &str) -> &str {
    s.split_once('|').map_or(s, |(text, _)| text)
}

/// Translate a fixed English string. A trailing `|context` lets homographs
/// keep distinct translation keys and is stripped when falling back to English.
pub fn tr(english: &'static str) -> &'static str {
    let raw = table(language()).and_then(|t| t.get(english).copied()).unwrap_or_else(|| strip_context(english));
    rebrand(raw)
}

/// Translate a string only known at runtime (e.g. an error from the engine).
pub fn tr_dyn(english: &str) -> String {
    let s = table(language()).and_then(|t| t.get(english).map(|s| s.to_string())).unwrap_or_else(|| strip_context(english).to_string());
    s.replace("SteamInRust", BRAND)
}

/// Translate and fill `{name}` placeholders.
pub fn trf(english: &'static str, args: &[(&str, String)]) -> String {
    let mut out = tr(english).to_string();
    for (k, v) in args {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

#[macro_export]
macro_rules! t {
    ($s:literal) => {
        $crate::tr($s)
    };
    ($s:literal, $($k:ident = $v:expr),+ $(,)?) => {
        $crate::trf($s, &[$((stringify!($k), ($v).to_string())),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_and_formats() {
        set_language(Lang::English);
        assert_eq!(t!("Sign in"), "Sign in");
        assert_eq!(t!("Playing {game}", game = "Dota 2"), "Playing Dota 2");
        assert_eq!(t!("Recommended|review"), "Recommended");
        set_language(Lang::Russian);
        assert_eq!(t!("Sign in"), "Войти");
        assert_eq!(t!("Playing {game}", game = "Dota 2"), "Играет в Dota 2");
        set_language(Lang::English);
    }

    /// Every `t!("...")` in the app, as written in the code.
    fn app_strings() -> Vec<String> {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut keys: Vec<String> = Vec::new();
        let mut stack = vec![base.join("../../apps/desktop/src"), base.join("../../apps/installer/src"), base.join("../ui/src"), base.join("../video/src")];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let src = std::fs::read_to_string(&p).unwrap();
                let parts: Vec<&str> = src.split("t!(\"").collect();
                for (i, part) in parts.iter().enumerate().skip(1) {
                    // Skip `format!(`, `print!(` and similar.
                    if parts[i - 1].chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let Some(end) = part.find('"') else { continue };
                    let key = &part[..end];
                    if !keys.iter().any(|k| k == key) {
                        keys.push(key.to_string());
                    }
                }
            }
        }
        keys
    }

    /// Every `t!("...")` in the app must have a translation in every language.
    #[test]
    fn every_language_covers_all_strings() {
        let keys = app_strings();
        for lang in Lang::ALL.into_iter().filter(|l| *l != Lang::English) {
            let table = table(lang).unwrap();
            let missing: Vec<&String> = keys.iter().filter(|k| !table.contains_key(k.as_str())).collect();
            assert!(missing.is_empty(), "missing {lang:?} strings:\n{}", missing.iter().map(|m| format!("\"{m}\" = \"\"")).collect::<Vec<_>>().join("\n"));
        }
    }

    /// A translation keeps the English text's `{placeholders}`, or the value
    /// would never be filled in (or the raw name would show).
    #[test]
    fn translations_keep_placeholders() {
        fn names(s: &str) -> Vec<&str> {
            let mut v: Vec<&str> = s.split('{').skip(1).filter_map(|p| p.split_once('}').map(|(n, _)| n)).collect();
            v.sort();
            v
        }
        let mut wrong = Vec::new();
        for lang in Lang::ALL.into_iter().filter(|l| *l != Lang::English) {
            for (key, value) in table(lang).unwrap() {
                if names(key) != names(value) {
                    wrong.push(format!("{lang:?}: \"{key}\" = \"{value}\""));
                }
            }
        }
        wrong.sort();
        assert!(wrong.is_empty(), "placeholders differ:\n{}", wrong.join("\n"));
    }
}
