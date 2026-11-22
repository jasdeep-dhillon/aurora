//! Steam Input bindings for a game (ISteamInput in our steamclient): the
//! controller_mappings VDF that maps an Xbox-style pad to the game's actions.
//! Looked up like Steam's client does: the player's own configuration
//! first, then the configuration the game ships, then the game's official
//! configurations (Workshop files listed in its app info), unless the player
//! explicitly picked a configuration in Settings → Controller.

use model::AppId;
use net::Connection;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
pub use vdf::controller::{Control, LayoutDocument, Output, key_name as controller_key_name};
use vdf::controller::MAX_BYTES;

mod storage;
pub(crate) mod community;
pub use community::{CommunityCursor, CommunityLayout, CommunityPage, LayoutRequest};

/// Pads we read through XInput, best match first.
const TYPES: &[&str] = &[
    "controller_xboxone",
    "controller_xbox360",
    "controller_xboxelite",
    "controller_generic",
    "controller_ps4",
    "controller_ps5",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingOrigin {
    UserFile,
    UserWorkshop(u64),
    GameFile,
    Official(u64),
    CustomFile,
    Template,
    Community(u64),
    Custom,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BindingChoice {
    #[default]
    Auto,
    LocalFile(PathBuf),
    Workshop(u64),
    Template(TemplateChoice),
    Community(u64),
    Custom(LayoutId),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TemplateChoice {
    BuiltIn(BuiltInTemplate),
    Installed(TemplateFileName),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BuiltInTemplate {
    Gamepad,
    Camera,
    Keyboard,
    Precision,
}

impl BuiltInTemplate {
    pub const ALL: [Self; 4] = [Self::Gamepad, Self::Camera, Self::Keyboard, Self::Precision];

    fn text(self) -> &'static str {
        match self {
            Self::Gamepad => include_str!("input_config/templates/gamepad.vdf"),
            Self::Camera => include_str!("input_config/templates/camera.vdf"),
            Self::Keyboard => include_str!("input_config/templates/keyboard.vdf"),
            Self::Precision => include_str!("input_config/templates/precision.vdf"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String")]
pub struct LayoutId(String);

impl TryFrom<String> for LayoutId {
    type Error = &'static str;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        if s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
            Ok(Self(s.to_ascii_lowercase()))
        } else {
            Err("expected a 32-digit hexadecimal layout ID")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String")]
pub struct TemplateFileName(String);

impl TryFrom<String> for TemplateFileName {
    type Error = &'static str;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        if s.len() <= 255 && s.len() > 4 && s.to_ascii_lowercase().ends_with(".vdf")
            && !s.starts_with(['.', ' '])
            && !s.chars().any(|c| c.is_control() || "/\\:<>|?*".contains(c) || c == '"')
        {
            Ok(Self(s))
        } else {
            Err("expected one .vdf basename")
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionSetSummary {
    pub name: String,
    pub title: String,
    pub is_layer: bool,
    /// `(source, input_or_mode, action_names)`
    pub bindings: Vec<(String, String, Vec<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingOption {
    pub choice: BindingChoice,
    pub origin: BindingOrigin,
    pub controller_type: String,
    pub label: String,
    pub path: Option<PathBuf>,
    pub workshop_id: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct InputConfigReport {
    pub app_id: AppId,
    pub active_choice: BindingChoice,
    pub active_source: String,
    pub active_path: Option<PathBuf>,
    pub active_workshop_id: Option<u64>,
    pub active_controller_type: String,
    pub action_sets: Vec<ActionSetSummary>,
    pub available: Vec<BindingOption>,
    pub config_dir: Option<PathBuf>,
    pub document: Option<LayoutDocument>,
}

#[derive(Default)]
struct Selections {
    choices: BTreeMap<AppId, BindingChoice>,
    revisions: HashMap<AppId, u64>,
}

static APP_BINDINGS: OnceLock<Mutex<Selections>> = OnceLock::new();

fn bindings_lock() -> &'static Mutex<Selections> {
    APP_BINDINGS.get_or_init(|| Mutex::new(Selections::default()))
}

pub fn app_bindings() -> BTreeMap<AppId, BindingChoice> {
    bindings_lock().lock().unwrap_or_else(|e| e.into_inner()).choices.clone()
}

pub fn set_app_bindings(map: BTreeMap<AppId, BindingChoice>) {
    let mut selections = bindings_lock().lock().unwrap_or_else(|e| e.into_inner());
    let changed: std::collections::HashSet<_> = selections.choices.keys().chain(map.keys()).copied().collect();
    for app_id in changed {
        *selections.revisions.entry(app_id).or_default() += 1;
    }
    selections.choices = map
        .into_iter()
        .filter(|(_, c)| !matches!(c, BindingChoice::Auto))
        .collect();
    drop(selections);
    crate::preferences_changed();
}

pub fn set_app_binding(app_id: AppId, choice: BindingChoice) {
    let mut guard = bindings_lock().lock().unwrap_or_else(|e| e.into_inner());
    if matches!(choice, BindingChoice::Auto) {
        guard.choices.remove(&app_id);
    } else {
        guard.choices.insert(app_id, choice);
    }
    *guard.revisions.entry(app_id).or_default() += 1;
    drop(guard);
    crate::preferences_changed();
}

pub fn app_binding(app_id: AppId) -> BindingChoice {
    selection(app_id).1
}

pub(crate) fn selection(app_id: AppId) -> (u64, BindingChoice) {
    let selected = bindings_lock().lock().unwrap_or_else(|e| e.into_inner());
    (selected.revisions.get(&app_id).copied().unwrap_or(0), selected.choices.get(&app_id).cloned().unwrap_or_default())
}

#[derive(Default)]
pub(crate) struct LaunchLayout {
    pub current: Option<(u64, ResolvedLayout)>,
    pub acknowledged: Option<u64>,
}

fn is_mappings(text: &str) -> bool {
    LayoutDocument::parse(text).is_ok()
}

pub fn read_layout(path: &Path) -> Result<LayoutDocument, String> {
    let text = storage::read_file(path).map_err(|error| error.to_string())?;
    LayoutDocument::parse(&text).map_err(|error| error.to_string())
}

pub fn save_custom_layout(account: u32, app_id: AppId, doc: &LayoutDocument) -> Result<BindingChoice, String> {
    let id = LayoutId(crate::ipc::os_random_hex(16));
    store_layout(account, app_id, "custom", &format!("{}.vdf", id.0), doc)?;
    Ok(BindingChoice::Custom(id))
}

pub fn managed_bindings_dir(account: u32, app_id: AppId) -> PathBuf {
    crate::store::config_dir()
        .join("controller-bindings")
        .join(account.to_string())
        .join(app_id.to_string())
}

fn managed(account: u32, app_id: AppId, kind: &str, create: bool) -> std::io::Result<storage::Directory> {
    storage::Directory::open(&crate::store::config_dir(),
        &["controller-bindings", &account.to_string(), &app_id.to_string(), kind], create)
}

fn store_layout(account: u32, app_id: AppId, kind: &str, file: &str, doc: &LayoutDocument) -> Result<(), String> {
    let text = doc.write();
    let checked = LayoutDocument::parse(&text).map_err(|err| err.to_string())?;
    let _compiled = checked.legacy_map();
    managed(account, app_id, kind, true).and_then(|dir| dir.commit(file, text.as_bytes())).map_err(|err| err.to_string())
}

fn templates(root: &Path) -> std::io::Result<storage::Directory> {
    if root.as_os_str().is_empty() {
        return Err(std::io::ErrorKind::NotFound.into());
    }
    storage::Directory::open(root, &["controller_base", "templates"], false)
}

fn files(dir: &storage::Directory) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(&dir.path).into_iter().flatten().take(256).flatten()
        .filter_map(|e| e.file_name().into_string().ok()).collect();
    names.sort();
    names
}

fn rank(text: &str) -> usize {
    TYPES.iter().position(|t| text.contains(&format!("\"{t}\""))).unwrap_or(TYPES.len())
}

fn controller_type_in(text: &str) -> String {
    vdf::parse(text)
        .ok()
        .and_then(|d| d.obj("controller_mappings")?.str("controller_type").map(str::to_string))
        .unwrap_or_default()
}

/// The player's Steam Controller Configs directory for one game.
pub fn player_config_dir(steam_root: &Path, account: u32, app_id: AppId) -> PathBuf {
    steam_root
        .join("steamapps")
        .join("common")
        .join("Steam Controller Configs")
        .join(account.to_string())
        .join("config")
        .join(app_id.to_string())
}

/// All Xbox-style / controller_mappings `.vdf` files under `dir`, best rank first.
fn all_in(dir: &Path, depth: u32) -> Vec<(usize, String, PathBuf)> {
    let mut found: Vec<(usize, String, PathBuf)> = Vec::new();
    fn walk(dir: &Path, depth: u32, budget: &mut usize, found: &mut Vec<(usize, String, PathBuf)>) {
        if !std::fs::symlink_metadata(dir).is_ok_and(|meta| meta.is_dir() && !storage::is_link(&meta)) {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            let p = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
            if storage::is_link(&meta) {
                continue;
            }
            if meta.is_dir() && depth > 0 {
                walk(&p, depth - 1, budget, found);
            } else if meta.is_file()
                && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("vdf"))
                && let Ok(t) = storage::read_file(&p)
                && is_mappings(&t)
            {
                found.push((rank(&t), t, p));
            }
        }
    }
    walk(dir, depth, &mut 256, &mut found);
    found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.cmp(&b.2)));
    found
}

/// The best Xbox-style mappings file under `dir`.
fn best_in(dir: &Path, depth: u32) -> Option<(String, PathBuf)> {
    all_in(dir, depth).into_iter().next().map(|(_, t, p)| (t, p))
}

/// The player's configurations: `<Steam>/steamapps/common/Steam Controller Configs/<account>/config/<app>/`.
fn player_config(steam_root: &Path, account: u32, app_id: AppId) -> Option<(String, PathBuf)> {
    let dir = player_config_dir(steam_root, account, app_id);
    best_in(&dir, 1)
}

/// Workshop configurations the player picked per controller type
/// (`config/configset_<vid>-<pid>-….vdf`: "controller_config" { "<app>" { "workshop" "<id>" } }),
/// Xbox pads (Microsoft, 045e) first.
fn player_workshop_choices(steam_root: &Path, account: u32, app_id: AppId, xbox_only: bool) -> Vec<(u64, String)> {
    let dir = steam_root
        .join("steamapps")
        .join("common")
        .join("Steam Controller Configs")
        .join(account.to_string())
        .join("config");
    let mut sets: Vec<(u8, String, PathBuf)> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.take(256).flatten()
                .map(|e| e.path())
                .filter_map(|p| {
                    let n = p.file_name()?.to_str()?.to_ascii_lowercase();
                    if !n.starts_with("configset_") || !n.ends_with(".vdf") {
                        return None;
                    }
                    let order = if n.starts_with("configset_45e-") {
                        0
                    } else if n.starts_with("configset_controller_generic") || n.starts_with("configset_controller_xbox") {
                        1
                    } else if xbox_only {
                        return None;
                    } else {
                        2
                    };
                    Some((order, n, p))
                })
                .collect()
        })
        .unwrap_or_default();
    sets.sort();
    sets.into_iter()
        .filter_map(|(_, n, p)| {
            let id = vdf::parse(&storage::read_file(&p).ok()?)
                .ok()?
                .obj("controller_config")?
                .obj(&app_id.to_string())?
                .u64("workshop")?;
            Some((id, n))
        })
        .collect()
}

/// A configuration from the Workshop (they belong to Steam's controller
/// configs app, 241100, not to the game).
async fn workshop_file(conn: &Connection, _app_id: AppId, id: u64) -> Option<String> {
    community::download_text(conn, id, None).await.ok()
}

/// Official configurations from the app info:
/// `config/steamcontrollerconfigdetails/<file id>/controller_type`.
async fn official_list(conn: &Connection, app_id: AppId) -> Vec<(usize, u64, String)> {
    let Some(info) = crate::pics::raw_app_info(conn, app_id).await else { return Vec::new() };
    let Some(details) = info.path(&["config", "steamcontrollerconfigdetails"]) else { return Vec::new() };
    let mut files: Vec<(usize, u64, String)> = details
        .iter()
        .filter_map(|(id, v)| {
            let kind = v.as_obj()?.str("controller_type").unwrap_or_default();
            let branches = v.as_obj()?.str("enabled_branches").unwrap_or("public");
            (branches.is_empty() || branches.split(',').any(|b| b.trim() == "public")).then_some(())?;
            let order = TYPES.iter().position(|t| *t == kind).unwrap_or(TYPES.len());
            Some((order, id.parse().ok()?, kind.to_string()))
        })
        .collect();
    files.sort_by_key(|f| f.0);
    files
}

#[derive(Default)]
pub(crate) struct ResolvedLayout {
    pub text: String,
    pub source: String,
    path: Option<PathBuf>,
    workshop_id: Option<u64>,
}

impl ResolvedLayout {
    fn file(text: String, path: PathBuf, label: &str) -> Self {
        let source = format!("{label} ({})", path.display());
        Self { text, source, path: Some(path), workshop_id: None }
    }
}

fn explicit_local(choice: &BindingChoice, steam_root: &Path, account: u32, app_id: AppId) -> Option<ResolvedLayout> {
    let (text, path, workshop_id) = match choice {
        BindingChoice::LocalFile(path) => {
            let text = storage::read_file(path).ok()?;
            (text, Some(path.clone()), None)
        }
        BindingChoice::Template(TemplateChoice::BuiltIn(template)) => (template.text().into(), None, None),
        BindingChoice::Template(TemplateChoice::Installed(name)) => {
            let dir = templates(steam_root).ok()?;
            (dir.read(&name.0).ok()?, Some(dir.path.join(&name.0)), None)
        }
        BindingChoice::Community(id) => {
            let dir = managed(account, app_id, "community", false).ok()?;
            let name = format!("{id}.vdf");
            (dir.read(&name).ok()?, Some(dir.path.join(name)), Some(*id))
        }
        BindingChoice::Custom(id) => {
            let dir = managed(account, app_id, "custom", false).ok()?;
            let name = format!("{}.vdf", id.0);
            (dir.read(&name).ok()?, Some(dir.path.join(name)), None)
        }
        BindingChoice::Auto | BindingChoice::Workshop(_) => return None,
    };
    let doc = LayoutDocument::parse(&text).ok()?;
    let source = doc.title().map(str::to_owned).or_else(|| {
        path.as_ref()?.file_name()?.to_str().map(str::to_owned)
    }).unwrap_or_default();
    Some(ResolvedLayout { text, source, path, workshop_id })
}

/// The canonical precedence for Settings, launch-time IPC and live updates.
/// With no online connection, skip remote candidates and use the same local order.
pub(crate) async fn resolve_layout(
    conn: Option<&Connection>, app_id: AppId, account: u32, steam_root: &Path, install_dir: &Path,
    choice: &BindingChoice, officials: Option<&[(usize, u64, String)]>,
) -> ResolvedLayout {
    if let Some(resolved) = explicit_local(choice, steam_root, account, app_id) {
        return resolved;
    }
    if let (BindingChoice::Workshop(id), Some(conn)) = (choice, conn)
        && let Some(text) = workshop_file(conn, app_id, *id).await
    {
        return ResolvedLayout { text, source: format!("Workshop #{id}"), workshop_id: Some(*id), path: None };
    }
    if !steam_root.as_os_str().is_empty() {
        if let Some((text, path)) = player_config(steam_root, account, app_id) {
            return ResolvedLayout::file(text, path, "User configuration");
        }
        if let Some(conn) = conn {
            for (id, _) in player_workshop_choices(steam_root, account, app_id, true).into_iter().take(8) {
                if let Some(text) = workshop_file(conn, app_id, id).await {
                    return ResolvedLayout { text, source: format!("User Workshop #{id}"), workshop_id: Some(id), path: None };
                }
            }
        }
    }
    if !install_dir.as_os_str().is_empty()
        && let Some((text, path)) = best_in(install_dir, 3)
    {
        return ResolvedLayout::file(text, path, "Game files");
    }
    if let Some(conn) = conn {
        let fetched;
        let list = match officials {
            Some(list) => list,
            None => {
                fetched = official_list(conn, app_id).await;
                &fetched
            }
        };
        for (_, id, _) in list.iter().take(8) {
            if let Some(text) = workshop_file(conn, app_id, *id).await {
                return ResolvedLayout { text, source: format!("Official Workshop #{id}"), workshop_id: Some(*id), path: None };
            }
        }
    }
    ResolvedLayout::default()
}

pub fn binding_document(choice: &BindingChoice, steam_root: &Path, account: u32, app_id: AppId) -> Option<LayoutDocument> {
    LayoutDocument::parse(&explicit_local(choice, steam_root, account, app_id)?.text).ok()
}

fn game_actions_in(input: &vdf::Obj) -> Vec<String> {
    let mut out = Vec::new();
    for (_, act) in input.obj("activators").map(|a| a.iter().collect::<Vec<_>>()).unwrap_or_default() {
        let Some(bindings) = act.as_obj().and_then(|a| a.obj("bindings")) else { continue };
        for (k, v) in bindings.iter() {
            let Some(b) = v.as_str().filter(|_| k == "binding") else { continue };
            let mut words = b.split(',').next().unwrap_or_default().split_whitespace();
            if words.next() == Some("game_action")
                && let (Some(_set), Some(action)) = (words.next(), words.next())
                && !out.iter().any(|existing| existing == action)
            {
                out.push(action.to_string());
            }
        }
    }
    out
}

/// Extract controller type and human-readable action sets / layers from a
/// `controller_mappings` VDF.
pub fn parse_vdf_summary(text: &str) -> (String, Vec<ActionSetSummary>) {
    let Ok(doc) = LayoutDocument::parse(text) else {
        return (String::new(), Vec::new());
    };
    let m = doc.mappings();
    let ctrl_type = m.str("controller_type").unwrap_or_default().to_string();

    let mut set_titles: HashMap<String, String> = HashMap::new();
    let mut layer_names: Vec<(String, String)> = Vec::new();
    if let Some(actions) = m.obj("actions") {
        for (name, v) in actions.iter() {
            let title = v.as_obj().and_then(|o| o.str("title")).unwrap_or(name).to_string();
            set_titles.insert(name.to_string(), title);
        }
    }
    if let Some(layers) = m.obj("action_layers") {
        for (name, v) in layers.iter() {
            let title = v.as_obj().and_then(|o| o.str("title")).unwrap_or(name).to_string();
            set_titles.insert(name.to_string(), title.clone());
            layer_names.push((name.to_string(), title));
        }
    }

    struct GroupInfo {
        mode: String,
        inputs: Vec<(String, Vec<String>)>,
        analog: Vec<String>,
    }
    let mut groups: HashMap<String, GroupInfo> = HashMap::new();
    let mut sets: Vec<ActionSetSummary> = Vec::new();

    for (k, v) in m.iter() {
        let Some(o) = v.as_obj() else { continue };
        match k {
            "group" => {
                let id = o.str("id").unwrap_or_default().to_string();
                let mode = o.str("mode").unwrap_or_default().to_string();
                let mut inputs = Vec::new();
                if let Some(inp) = o.obj("inputs") {
                    for (name, item) in inp.iter() {
                        if let Some(item) = item.as_obj() {
                            let acts = game_actions_in(item);
                            if !acts.is_empty() {
                                inputs.push((name.to_string(), acts));
                            }
                        }
                    }
                }
                let analog = o
                    .obj("gameactions")
                    .map(|ga| ga.iter().filter_map(|(_, a)| a.as_str()).map(str::to_string).collect())
                    .unwrap_or_default();
                groups.insert(id, GroupInfo { mode, inputs, analog });
            }
            "preset" => {
                let name = o.str("name").unwrap_or_default().to_string();
                let is_layer = layer_names.iter().any(|(n, _)| n == &name);
                let title = set_titles.get(&name).cloned().unwrap_or_else(|| name.clone());
                let mut bindings = Vec::new();
                if let Some(gsb) = o.obj("group_source_bindings") {
                    for (gid, val) in gsb.iter() {
                        let Some(raw) = val.as_str() else { continue };
                        let mut w = raw.split_whitespace();
                        let Some(source) = w.next() else { continue };
                        if w.next() != Some("active") {
                            continue;
                        }
                        if let Some(g) = groups.get(gid) {
                            if !g.analog.is_empty() {
                                bindings.push((source.to_string(), g.mode.clone(), g.analog.clone()));
                            }
                            for (input_name, acts) in &g.inputs {
                                bindings.push((source.to_string(), input_name.clone(), acts.clone()));
                            }
                        }
                    }
                }
                sets.push(ActionSetSummary { name, title, is_layer, bindings });
            }
            _ => {}
        }
    }

    (ctrl_type, sets)
}

/// Inspect local Steam Input configurations for `app_id` without needing network.
pub fn inspect_local(steam_root: &Path, account: u32, app_id: AppId, install_dir: &Path) -> InputConfigReport {
    let mut available: Vec<BindingOption> = Vec::new();
    let cfg_dir = (!steam_root.as_os_str().is_empty()).then(|| player_config_dir(steam_root, account, app_id));

    if let Some(dir) = &cfg_dir {
        for (_, text, path) in all_in(dir, 1) {
            let ct = controller_type_in(&text);
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            available.push(BindingOption {
                choice: BindingChoice::LocalFile(path.clone()),
                origin: BindingOrigin::UserFile,
                controller_type: ct,
                label: format!("User: {name}"),
                path: Some(path.clone()),
                workshop_id: None,
            });
        }
    }

    if !steam_root.as_os_str().is_empty() {
        for (id, file) in player_workshop_choices(steam_root, account, app_id, false) {
            if !available.iter().any(|o| o.workshop_id == Some(id)) {
                available.push(BindingOption {
                    choice: BindingChoice::Workshop(id),
                    origin: BindingOrigin::UserWorkshop(id),
                    controller_type: file,
                    label: format!("Workshop #{id}"),
                    path: None,
                    workshop_id: Some(id),
                });
            }
        }
    }

    if !install_dir.as_os_str().is_empty() {
        for (_, text, path) in all_in(install_dir, 3) {
            let ct = controller_type_in(&text);
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            available.push(BindingOption {
                choice: BindingChoice::LocalFile(path.clone()),
                origin: BindingOrigin::GameFile,
                controller_type: ct,
                label: format!("Game: {name}"),
                path: Some(path.clone()),
                workshop_id: None,
            });
        }
    }

    available.extend(managed_options(steam_root, account, app_id));
    let active_choice = app_binding(app_id);
    if let BindingChoice::LocalFile(path) = &active_choice
        && !available.iter().any(|o| o.path.as_ref() == Some(path))
    {
        let label = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        available.push(BindingOption {
            choice: active_choice.clone(), origin: BindingOrigin::CustomFile,
            controller_type: read_layout(path).ok().and_then(|doc| doc.mappings().str("controller_type").map(str::to_owned)).unwrap_or_default(),
            label: label.into(), path: Some(path.clone()), workshop_id: None,
        });
    }
    let mut report = InputConfigReport { app_id, active_choice, available, config_dir: cfg_dir, ..Default::default() };
    // With no connection every branch completes immediately.
    use futures_util::FutureExt;
    let resolved = resolve_layout(None, app_id, account, steam_root, install_dir, &report.active_choice, None)
        .now_or_never().expect("local resolution has no asynchronous operations");
    populate(&mut report, resolved);
    report
}

fn is_desktop_steam_template(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    !(lower.starts_with("controller_android_")
        || lower.starts_with("controller_apple_")
        || lower.starts_with("controller_mobile_touch_")
        || lower.contains("touchscreen")
        || lower.contains("webbrowser"))
}

fn steam_template_rank(file_name: &str) -> u8 {
    let lower = file_name.to_ascii_lowercase();
    if !lower.starts_with("controller_") {
        0 // Generic root templates (gamepad_*.vdf, wasd.vdf)
    } else if lower.starts_with("controller_xboxone_") || lower.starts_with("controller_xbox360_") {
        1
    } else if lower.starts_with("controller_generic_") {
        2
    } else if lower.starts_with("controller_ps5_") || lower.starts_with("controller_ps4_") {
        3
    } else if lower.starts_with("controller_switch_pro_") {
        4
    } else if lower.starts_with("controller_neptune_") {
        5
    } else {
        6
    }
}

pub fn managed_options(steam_root: &Path, account: u32, app_id: AppId) -> Vec<BindingOption> {
    let mut options = Vec::new();
    let mut seen_template_titles = std::collections::HashSet::new();
    let mut add = |choice: BindingChoice, origin: BindingOrigin| {
        if let Some(resolved) = explicit_local(&choice, steam_root, account, app_id) {
            if origin == BindingOrigin::Template && !seen_template_titles.insert(resolved.source.to_ascii_lowercase()) {
                return;
            }
            options.push(BindingOption {
                choice, origin, controller_type: controller_type_in(&resolved.text), label: resolved.source,
                path: resolved.path, workshop_id: resolved.workshop_id,
            });
        }
    };
    for template in BuiltInTemplate::ALL {
        add(BindingChoice::Template(TemplateChoice::BuiltIn(template)), BindingOrigin::Template);
    }
    if let Ok(dir) = templates(steam_root) {
        let mut names: Vec<_> = files(&dir)
            .into_iter()
            .filter(|n| is_desktop_steam_template(n))
            .filter_map(|n| TemplateFileName::try_from(n).ok())
            .collect();
        names.sort_by(|a, b| steam_template_rank(&a.0).cmp(&steam_template_rank(&b.0)).then_with(|| a.0.cmp(&b.0)));
        for name in names {
            add(BindingChoice::Template(TemplateChoice::Installed(name)), BindingOrigin::Template);
        }
    }
    if let Ok(dir) = managed(account, app_id, "community", false) {
        for name in files(&dir) {
            if let Some(id) = name.strip_suffix(".vdf").and_then(|n| n.parse::<u64>().ok()).filter(|id| *id != 0) {
                add(BindingChoice::Community(id), BindingOrigin::Community(id));
            }
        }
    }
    if let Ok(dir) = managed(account, app_id, "custom", false) {
        for name in files(&dir) {
            if let Some(id) = name.strip_suffix(".vdf").and_then(|n| LayoutId::try_from(n.to_owned()).ok()) {
                add(BindingChoice::Custom(id), BindingOrigin::Custom);
            }
        }
    }
    options
}

fn populate(report: &mut InputConfigReport, resolved: ResolvedLayout) {
    let (kind, sets) = parse_vdf_summary(&resolved.text);
    report.active_source = resolved.source;
    report.active_path = resolved.path;
    report.active_workshop_id = resolved.workshop_id;
    report.active_controller_type = kind;
    report.action_sets = sets;
    report.document = LayoutDocument::parse(&resolved.text).ok();
}

/// Inspect local and online (Workshop + official PICS) Steam Input configurations.
pub async fn inspect(conn: &Connection, app_id: AppId, me: u64, steam_root: &Path, install_dir: &Path) -> InputConfigReport {
    let account = (me & 0xFFFF_FFFF) as u32;
    let mut report = inspect_local(steam_root, account, app_id, install_dir);
    let officials = official_list(conn, app_id).await;
    for (_, id, kind) in &officials {
        if !report.available.iter().any(|o| o.workshop_id == Some(*id)) {
            report.available.push(BindingOption {
                choice: BindingChoice::Workshop(*id),
                origin: BindingOrigin::Official(*id),
                controller_type: kind.clone(),
                label: if kind.is_empty() { format!("Official Workshop #{id}") } else { format!("Official ({kind}) #{id}") },
                path: None,
                workshop_id: Some(*id),
            });
        }
    }

    let resolved = resolve_layout(Some(conn), app_id, account, steam_root, install_dir, &report.active_choice, Some(&officials)).await;
    populate(&mut report, resolved);
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_VDF: &str = r##""controller_mappings"
{
	"version"		"3"
	"controller_type"		"controller_xboxone"
	"actions" { "InGame" { "title" "In-Game" } "Menu" { "title" "Menu" } }
	"action_layers" { "Aim" { "title" "Aim Layer" } }
	"group" { "id" "0" "mode" "four_buttons" "inputs" { "button_a" { "activators" { "Full_Press" { "bindings" { "binding" "game_action InGame jump, Jump, " } } } } } }
	"group" { "id" "1" "mode" "joystick_move" "gameactions" { "InGame" "Move" } }
	"group" { "id" "2" "mode" "four_buttons" "inputs" { "button_a" { "activators" { "Full_Press" { "bindings" { "binding" "game_action Menu select, , " } } } } } }
	"preset" { "id" "0" "name" "InGame" "group_source_bindings" { "0" "button_diamond active" "1" "left_joystick active" } }
	"preset" { "id" "1" "name" "Menu" "group_source_bindings" { "2" "button_diamond active" } }
	"preset" { "id" "2" "name" "Aim" "group_source_bindings" { "2" "button_diamond active" } }
}"##;

    #[test]
    fn prefers_xbox_mappings() {
        let dir = std::env::temp_dir().join("sir-input-config-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("cfg")).unwrap();
        std::fs::write(dir.join("cfg").join("ps4.vdf"), "\"controller_mappings\" { \"controller_type\" \"controller_ps4\" \"binding\" \"game_action set a\" }").unwrap();
        std::fs::write(dir.join("cfg").join("xbox.vdf"), "\"controller_mappings\" { \"controller_type\" \"controller_xboxone\" \"binding\" \"game_action set a\" }").unwrap();
        std::fs::write(dir.join("other.vdf"), "\"AppState\" {}").unwrap();
        let (_, p) = best_in(&dir, 2).unwrap();
        assert!(p.ends_with("xbox.vdf"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_action_sets_layers_and_inspects_local_configs() {
        let (ct, sets) = parse_vdf_summary(SAMPLE_VDF);
        assert_eq!(ct, "controller_xboxone");
        assert_eq!(sets.len(), 3);
        assert_eq!(sets[0].name, "InGame");
        assert_eq!(sets[0].title, "In-Game");
        assert!(!sets[0].is_layer);
        assert_eq!(sets[0].bindings.len(), 2);
        assert_eq!(sets[2].name, "Aim");
        assert!(sets[2].is_layer);

        let root = std::env::temp_dir().join(format!("sir-input-inspect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let steam = root.join("steam");
        let game = root.join("game");
        let user_dir = player_config_dir(&steam, 42, 620);
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&game).unwrap();
        let user_vdf = user_dir.join("my_pad.vdf");
        let game_vdf = game.join("default.vdf");
        std::fs::write(&user_vdf, SAMPLE_VDF).unwrap();
        std::fs::write(&game_vdf, SAMPLE_VDF).unwrap();

        set_app_binding(620, BindingChoice::Auto);
        let report = inspect_local(&steam, 42, 620, &game);
        assert_eq!(report.available.len(), 6);
        assert_eq!(report.active_path.as_ref(), Some(&user_vdf));
        assert_eq!(report.action_sets.len(), 3);

        set_app_binding(620, BindingChoice::LocalFile(game_vdf.clone()));
        let picked = inspect_local(&steam, 42, 620, &game);
        assert_eq!(picked.active_path.as_ref(), Some(&game_vdf));

        set_app_binding(620, BindingChoice::Auto);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolves_localized_template_titles_and_deduplicates_steam_templates() {
        let root = std::env::temp_dir().join(format!("sir-input-templates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tdir = root.join("controller_base").join("templates");
        std::fs::create_dir_all(&tdir).unwrap();
        let make_tpl = |en_title: &str| {
            format!(
                r##""controller_mappings" {{ "version" "3" "title" "#Title" "controller_type" "controller_xboxone" "localization" {{ "english" {{ "title" "{en_title}" }} }} }}"##
            )
        };
        // Duplicate of built-in "Gamepad" should be deduplicated out.
        std::fs::write(tdir.join("controller_xboxone_gamepad_joystick.vdf"), make_tpl("Gamepad")).unwrap();
        // Mobile template should be filtered out.
        std::fs::write(tdir.join("controller_android_gamepad_fps.vdf"), make_tpl("Android Pad")).unwrap();
        // Two vendor copies of Flick Stick should collapse to one clean localized entry.
        std::fs::write(tdir.join("controller_ps4_gamepad_flickstick.vdf"), make_tpl("Gamepad with Flick Stick")).unwrap();
        std::fs::write(tdir.join("controller_ps5_gamepad_flickstick.vdf"), make_tpl("Gamepad with Flick Stick")).unwrap();

        let opts = managed_options(&root, 42, 553850);
        let labels: Vec<&str> = opts.iter().map(|o| o.label.as_str()).collect();
        assert!(!labels.iter().any(|l| l.contains("#Title")), "raw #Title leaked: {labels:?}");
        assert!(!labels.iter().any(|l| *l == "Android Pad"), "mobile template leaked: {labels:?}");
        assert_eq!(labels.iter().filter(|l| **l == "Gamepad").count(), 1, "duplicate Gamepad: {labels:?}");
        assert_eq!(
            labels.iter().filter(|l| **l == "Gamepad with Flick Stick").count(),
            1,
            "duplicate Flick Stick: {labels:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
