//! Steam Play compatibility tools on Linux (Proton and friends): which ones
//! are installed, which one a game is set to use, and the Steam Linux
//! Runtime a tool asks to run inside. Read from the Steam install, the same
//! files Steam itself uses.

use model::AppId;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct CompatTool {
    /// Internal name, as in config.vdf's CompatToolMapping (`proton_11`,
    /// `GE-Proton10-34`, `proton-cachyos-native`).
    pub name: String,
    /// Name to show (`Proton 11.0`).
    pub display: String,
    /// Folder holding the `proton` script.
    pub dir: PathBuf,
    /// Steam Linux Runtime app the tool asks to run inside (toolmanifest.vdf
    /// `require_tool_appid`), if any.
    pub runtime_app: Option<AppId>,
}

impl CompatTool {
    pub fn script(&self) -> PathBuf {
        self.dir.join("proton")
    }
}

fn read_vdf(path: &Path) -> Option<vdf::Obj> {
    vdf::parse(&std::fs::read_to_string(path).ok()?).ok()
}

fn runtime_of(dir: &Path) -> Option<AppId> {
    let m = read_vdf(&dir.join("toolmanifest.vdf"))?;
    m.obj("manifest")?.u64("require_tool_appid").map(|v| v as AppId)
}

/// Every installed tool: Valve's Proton apps in the Steam libraries, then
/// custom tools in `compatibilitytools.d` (the user's and the system's).
pub fn compat_tools(root: &Path) -> Vec<CompatTool> {
    let mut out = Vec::new();
    for lib in crate::library_folders(root).unwrap_or_default() {
        let apps = lib.join("steamapps");
        let Ok(entries) = std::fs::read_dir(&apps) else { continue };
        for e in entries.flatten() {
            let file = e.file_name().to_string_lossy().to_string();
            if !(file.starts_with("appmanifest_") && file.ends_with(".acf")) {
                continue;
            }
            let Some(m) = read_vdf(&e.path()) else { continue };
            let Some(state) = m.obj("AppState") else { continue };
            let name = state.str("name").unwrap_or_default();
            let Some(internal) = valve_internal_name(name) else { continue };
            let dir = apps.join("common").join(state.str("installdir").unwrap_or_default());
            if dir.join("proton").is_file() {
                out.push(CompatTool { name: internal, display: name.to_string(), runtime_app: runtime_of(&dir), dir });
            }
        }
    }
    let mut dirs = vec![root.join("compatibilitytools.d")];
    dirs.push(PathBuf::from("/usr/share/steam/compatibilitytools.d"));
    dirs.push(PathBuf::from("/usr/local/share/steam/compatibilitytools.d"));
    for base in dirs {
        let Ok(entries) = std::fs::read_dir(&base) else { continue };
        for e in entries.flatten() {
            let folder = e.path();
            let Some(m) = read_vdf(&folder.join("compatibilitytool.vdf")) else { continue };
            let Some(tools) = m.obj("compatibilitytools").and_then(|c| c.obj("compat_tools")) else { continue };
            for (name, v) in tools.iter() {
                let Some(t) = v.as_obj() else { continue };
                let install = t.str("install_path").unwrap_or(".");
                let dir = if install == "." { folder.clone() } else { folder.join(install) };
                if !dir.join("proton").is_file() || out.iter().any(|x: &CompatTool| x.name == name) {
                    continue;
                }
                out.push(CompatTool { name: name.to_string(), display: t.str("display_name").unwrap_or(name).to_string(), runtime_app: runtime_of(&dir), dir });
            }
        }
    }
    out
}

/// Steam's internal name for one of its own Proton apps, from the app name:
/// "Proton 11.0" -> proton_11, "Proton - Experimental" -> proton_experimental,
/// "Proton 9.0 (Beta)" -> proton_9, "Proton Hotfix" -> proton_hotfix.
fn valve_internal_name(app_name: &str) -> Option<String> {
    let rest = app_name.strip_prefix("Proton")?.trim();
    let lower = rest.trim_start_matches('-').trim().to_lowercase();
    if lower.starts_with("experimental") {
        return Some("proton_experimental".into());
    }
    if lower.starts_with("hotfix") {
        return Some("proton_hotfix".into());
    }
    let major: String = lower.chars().take_while(|c| c.is_ascii_digit()).collect();
    if major.is_empty() {
        return None;
    }
    // Proton 3.x and 4.x used dotted names (proton_316, proton_411).
    let minor: String = lower[major.len()..].trim_start_matches('.').chars().take_while(|c| c.is_ascii_digit()).collect();
    match major.parse::<u32>() {
        Ok(m) if m < 5 && !minor.is_empty() => Some(format!("proton_{major}{minor}")),
        _ => Some(format!("proton_{major}")),
    }
}

/// The tool set for `app` in Steam (Properties -> Compatibility), and
/// whether it was chosen for this game specifically (`false` = the global
/// default for all games).
fn mapped_tool(root: &Path, app: AppId) -> Option<(String, bool)> {
    let cfg = read_vdf(&root.join("config").join("config.vdf"))?;
    let map = cfg.path(&["InstallConfigStore", "Software", "Valve", "Steam", "CompatToolMapping"])?;
    let name_of = |key: &str| map.obj(key).and_then(|o| o.str("name")).filter(|n| !n.is_empty()).map(String::from);
    name_of(&app.to_string()).map(|n| (n, true)).or_else(|| name_of("0").map(|n| (n, false)))
}

/// Whether the player forced a tool on this game in Steam (e.g. Proton for
/// a game that also has a Linux version).
pub fn forced_tool(root: &Path, app: AppId) -> bool {
    mapped_tool(root, app).is_some_and(|(_, own)| own)
}

/// The tool to run `app` with: its own choice in Steam, else Steam's
/// default, else the best installed Proton (Experimental, then the newest).
pub fn tool_for(root: &Path, app: AppId) -> Option<CompatTool> {
    let tools = compat_tools(root);
    if let Some((name, _)) = mapped_tool(root, app) {
        if let Some(t) = tools.iter().find(|t| t.name.eq_ignore_ascii_case(&name)) {
            return Some(t.clone());
        }
    }
    let rank = |t: &CompatTool| -> (u8, u32) {
        if t.name == "proton_experimental" {
            return (2, 0);
        }
        let n = t.name.strip_prefix("proton_").and_then(|v| v.parse().ok()).unwrap_or(0);
        (u8::from(n > 0), n)
    };
    tools.into_iter().max_by_key(rank)
}

/// Install folder of any app (e.g. a Steam Linux Runtime), from its
/// appmanifest in whichever library holds it.
pub fn app_dir(root: &Path, app: AppId) -> Option<PathBuf> {
    crate::library_folders(root).unwrap_or_default().into_iter().find_map(|lib| {
        let apps = lib.join("steamapps");
        let m = read_vdf(&apps.join(format!("appmanifest_{app}.acf")))?;
        let dir = apps.join("common").join(m.obj("AppState")?.str("installdir")?);
        dir.is_dir().then_some(dir)
    })
}

/// The game's Proton prefix folder, `<library>/steamapps/compatdata/<app>`
/// (Steam keeps it in the library the game is installed in).
pub fn compat_data(install_dir: &Path, app: AppId) -> Option<PathBuf> {
    let steamapps = install_dir.parent()?.parent()?;
    Some(steamapps.join("compatdata").join(app.to_string()))
}

/// The Windows user folder inside a game's Proton prefix, where its saves
/// go (`…/pfx/drive_c/users/steamuser`), if the prefix exists.
pub fn prefix_user_dir(install_dir: &Path, app: AppId) -> Option<PathBuf> {
    let dir = compat_data(install_dir, app)?.join("pfx").join("drive_c").join("users").join("steamuser");
    dir.is_dir().then_some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valve_names() {
        assert_eq!(valve_internal_name("Proton 11.0").as_deref(), Some("proton_11"));
        assert_eq!(valve_internal_name("Proton 9.0 (Beta)").as_deref(), Some("proton_9"));
        assert_eq!(valve_internal_name("Proton - Experimental").as_deref(), Some("proton_experimental"));
        assert_eq!(valve_internal_name("Proton Hotfix").as_deref(), Some("proton_hotfix"));
        assert_eq!(valve_internal_name("Proton 3.16").as_deref(), Some("proton_316"));
        assert_eq!(valve_internal_name("Proton EasyAntiCheat Runtime"), None);
        assert_eq!(valve_internal_name("Steam Linux Runtime 3.0 (sniper)"), None);
    }

    /// Reads this machine's Steam install when there is one (no assertions
    /// that depend on what's installed).
    #[test]
    #[ignore]
    fn lists_installed_tools() {
        let Some(root) = crate::find_steam_root() else { return };
        for t in compat_tools(&root) {
            println!("{} | {} | {} | runtime {:?}", t.name, t.display, t.dir.display(), t.runtime_app);
        }
        println!("default for 0: {:?}", tool_for(&root, 0).map(|t| t.name));
    }
}
