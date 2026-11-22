//! Steam Input bindings for a game (ISteamInput in our steamclient): the
//! controller_mappings VDF that maps an Xbox-style pad to the game's actions.
//! Looked up like Steam's client does: the player's own configuration
//! first, then the configuration the game ships, then the game's official
//! configurations (Workshop files listed in its app info), unless the player
//! explicitly picked a configuration in Settings → Controller.

use crate::ipc::protocol::Reply;
use model::AppId;
use net::Connection;
use proto::*;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BindingChoice {
    #[default]
    Auto,
    LocalFile(PathBuf),
    Workshop(u64),
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
}

static APP_BINDINGS: OnceLock<Mutex<BTreeMap<AppId, BindingChoice>>> = OnceLock::new();

fn bindings_lock() -> &'static Mutex<BTreeMap<AppId, BindingChoice>> {
    APP_BINDINGS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub fn app_bindings() -> BTreeMap<AppId, BindingChoice> {
    bindings_lock().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn set_app_bindings(map: BTreeMap<AppId, BindingChoice>) {
    *bindings_lock().lock().unwrap_or_else(|e| e.into_inner()) = map
        .into_iter()
        .filter(|(_, c)| !matches!(c, BindingChoice::Auto))
        .collect();
    crate::preferences_changed();
}

pub fn set_app_binding(app_id: AppId, choice: BindingChoice) {
    let mut guard = bindings_lock().lock().unwrap_or_else(|e| e.into_inner());
    if matches!(choice, BindingChoice::Auto) {
        guard.remove(&app_id);
    } else {
        guard.insert(app_id, choice);
    }
    crate::preferences_changed();
}

pub fn app_binding(app_id: AppId) -> BindingChoice {
    bindings_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&app_id)
        .cloned()
        .unwrap_or(BindingChoice::Auto)
}

fn is_mappings(text: &str) -> bool {
    text.contains("controller_mappings") && text.contains("game_action")
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
    const MAX_SCAN_FILES: usize = 256;
    let Ok(root_meta) = std::fs::symlink_metadata(dir) else { return Vec::new() };
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return Vec::new();
    }
    let mut found: Vec<(usize, String, PathBuf)> = Vec::new();
    let mut scanned = 0usize;
    fn walk(dir: &Path, depth: u32, scanned: &mut usize, found: &mut Vec<(usize, String, PathBuf)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            if *scanned >= MAX_SCAN_FILES {
                return;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let p = e.path();
            if ft.is_dir() {
                if depth > 0 {
                    walk(&p, depth - 1, scanned, found);
                }
            } else if ft.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("vdf")) {
                *scanned += 1;
                if let Some(t) = read_bounded_config_file(&p)
                    && is_mappings(&t)
                {
                    found.push((rank(&t), t, p));
                }
            }
        }
    }
    walk(dir, depth, &mut scanned, &mut found);
    found.sort_by_key(|f| f.0);
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
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|ft| !ft.is_symlink() && ft.is_file()))
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
            let text = read_bounded_config_file(&p)?;
            let id = vdf::parse(&text)
                .ok()?
                .obj("controller_config")?
                .obj(&app_id.to_string())?
                .u64("workshop")?;
            Some((id, n))
        })
        .collect()
}

const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;
const WORKSHOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

fn is_trusted_steam_ugc_host(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    matches!(
        h.as_str(),
        "steamusercontent-a.akamaihd.net"
            | "steamuserimages-a.akamaihd.net"
            | "steamcdn-a.akamaihd.net"
            | "media.steampowered.com"
            | "store.steampowered.com"
            | "steamcommunity.com"
    ) || h.ends_with(".steamusercontent.com")
        || h.ends_with(".steamstatic.com")
        || h.ends_with(".steamcontent.com")
}

fn validate_workshop_config_url(url: &str) -> Result<reqwest::Url, String> {
    if url.is_empty() || url.contains('\\') || url.chars().any(|c| c.is_control()) {
        return Err("invalid Workshop config URL".into());
    }
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid Workshop config URL: {e}"))?;
    if parsed.scheme() != "https" {
        return Err("refusing non-HTTPS Workshop config URL".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("refusing credentials in Workshop config URL".into());
    }
    let host = parsed.host_str().filter(|h| !h.is_empty()).ok_or_else(|| "missing host in Workshop config URL".to_string())?;
    if !is_trusted_steam_ugc_host(host) {
        return Err(format!("refusing untrusted Workshop config host: {host}"));
    }
    Ok(parsed)
}

fn workshop_http() -> &'static reqwest::Client {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(WORKSHOP_TIMEOUT)
            .connect_timeout(std::time::Duration::from_secs(10))
            .https_only(true)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    attempt.error("too many redirects")
                } else if attempt.url().scheme() != "https" {
                    attempt.error("refusing non-HTTPS redirect")
                } else if attempt.url().host_str().is_none_or(|h| !is_trusted_steam_ugc_host(h)) {
                    attempt.error("refusing redirect to untrusted host")
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .unwrap_or_default()
    })
}

fn read_bounded_config_file(path: &Path) -> Option<String> {
    use std::io::Read;
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return None;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        opts.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let f = opts.open(path).ok()?;
    let fmeta = f.metadata().ok()?;
    if !fmeta.is_file() {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if (fmeta.file_attributes() & 0x0400) != 0 {
            return None;
        }
    }
    let mut buf = Vec::new();
    f.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > MAX_CONFIG_BYTES {
        tracing::warn!("input_config: refusing local config exceeding {} bytes: {}", MAX_CONFIG_BYTES, path.display());
        return None;
    }
    String::from_utf8(buf).ok()
}

/// A configuration from the Workshop (they belong to Steam's controller
/// configs app, 241100, not to the game).
async fn workshop_file(conn: &Connection, _app_id: AppId, id: u64) -> Option<String> {
    let req = CPublishedFileGetDetailsRequest { publishedfileids: vec![id], ..Default::default() };
    let r = conn
        .call::<_, CPublishedFileGetDetailsResponse>("PublishedFile.GetDetails#1", &req)
        .await
        .ok()?;
    let d = r.publishedfiledetails.first()?;
    let url = validate_workshop_config_url(d.file_url()).ok()?;
    let mut resp = workshop_http().get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    if resp.content_length().is_some_and(|n| n > MAX_CONFIG_BYTES) {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        if (bytes.len() as u64).saturating_add(chunk.len() as u64) > MAX_CONFIG_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(bytes).ok()?;
    is_mappings(&text).then_some(text)
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

async fn official(conn: &Connection, app_id: AppId) -> Option<(String, u64)> {
    for (_, id, _) in official_list(conn, app_id).await {
        if let Some(text) = workshop_file(conn, app_id, id).await {
            return Some((text, id));
        }
    }
    None
}

pub async fn config(conn: &Connection, app_id: AppId, me: u64, steam_root: &Path, install_dir: &Path) -> Reply {
    let account = (me & 0xFFFF_FFFF) as u32;
    let override_choice = app_binding(app_id);
    let chosen = match &override_choice {
        BindingChoice::LocalFile(path) => read_bounded_config_file(path)
            .filter(|t| is_mappings(t))
            .map(|t| (t, format!("selected file ({})", path.display()))),
        BindingChoice::Workshop(id) => workshop_file(conn, app_id, *id)
            .await
            .map(|t| (t, format!("selected Workshop configuration ({id})"))),
        BindingChoice::Auto => None,
    };

    let found = if chosen.is_some() {
        chosen
    } else if let Some((t, p)) = player_config(steam_root, account, app_id) {
        Some((t, format!("your configuration ({})", p.display())))
    } else if let Some((t, id)) = async {
        for (id, _) in player_workshop_choices(steam_root, account, app_id, true) {
            if let Some(t) = workshop_file(conn, app_id, id).await {
                return Some((t, id));
            }
        }
        None
    }
    .await
    {
        Some((t, format!("your Workshop configuration ({id})")))
    } else if let Some((t, p)) = (!install_dir.as_os_str().is_empty()).then(|| best_in(install_dir, 3)).flatten() {
        Some((t, format!("the game's files ({})", p.display())))
    } else {
        official(conn, app_id)
            .await
            .map(|(t, id)| (t, format!("official configuration (Workshop {id})")))
    };

    match found {
        Some((bindings, source)) => {
            tracing::info!("Steam Input for {app_id}: {source}");
            Reply::InputConfig { bindings, source }
        }
        None => {
            tracing::info!("Steam Input for {app_id}: no bindings for an Xbox-style pad");
            Reply::InputConfig { bindings: String::new(), source: String::new() }
        }
    }
}

const MAX_SUMMARY_SETS: usize = 64;
const MAX_SUMMARY_BINDINGS_PER_SET: usize = 256;
const MAX_SUMMARY_TOTAL_BINDINGS: usize = 2048;
const MAX_SUMMARY_TOTAL_ACTIONS: usize = 4096;
const MAX_ACTIONS_PER_INPUT: usize = 64;

fn game_actions_in(input: &vdf::Obj) -> Vec<String> {
    let mut out = Vec::new();
    for (_, act) in input.obj("activators").map(|a| a.iter().collect::<Vec<_>>()).unwrap_or_default() {
        let Some(bindings) = act.as_obj().and_then(|a| a.obj("bindings")) else { continue };
        for (k, v) in bindings.iter() {
            if out.len() >= MAX_ACTIONS_PER_INPUT {
                return out;
            }
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
    let Ok(doc) = vdf::parse(text) else {
        return (String::new(), Vec::new());
    };
    let Some(m) = doc.obj("controller_mappings") else {
        return (String::new(), Vec::new());
    };
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
    let mut total_bindings = 0usize;
    let mut total_actions = 0usize;

    for (k, v) in m.iter() {
        let Some(o) = v.as_obj() else { continue };
        match k {
            "group" => {
                let id = o.str("id").unwrap_or_default().to_string();
                let mode = o.str("mode").unwrap_or_default().to_string();
                let mut inputs = Vec::new();
                if let Some(inp) = o.obj("inputs") {
                    for (name, item) in inp.iter() {
                        if inputs.len() >= MAX_SUMMARY_BINDINGS_PER_SET {
                            break;
                        }
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
                    .map(|ga| ga.iter().filter_map(|(_, a)| a.as_str()).take(MAX_ACTIONS_PER_INPUT).map(str::to_string).collect())
                    .unwrap_or_default();
                groups.insert(id, GroupInfo { mode, inputs, analog });
            }
            "preset" => {
                if sets.len() >= MAX_SUMMARY_SETS {
                    break;
                }
                let name = o.str("name").unwrap_or_default().to_string();
                let is_layer = layer_names.iter().any(|(n, _)| n == &name);
                let title = set_titles.get(&name).cloned().unwrap_or_else(|| name.clone());
                let mut bindings = Vec::new();
                let mut seen_gids = std::collections::HashSet::new();
                if let Some(gsb) = o.obj("group_source_bindings") {
                    'gsb: for (gid, val) in gsb.iter() {
                        if !seen_gids.insert(gid) {
                            continue;
                        }
                        let Some(raw) = val.as_str() else { continue };
                        let mut w = raw.split_whitespace();
                        let Some(source) = w.next() else { continue };
                        if w.next() != Some("active") {
                            continue;
                        }
                        if let Some(g) = groups.get(gid) {
                            if !g.analog.is_empty() {
                                if bindings.len() >= MAX_SUMMARY_BINDINGS_PER_SET
                                    || total_bindings >= MAX_SUMMARY_TOTAL_BINDINGS
                                    || total_actions.saturating_add(g.analog.len()) > MAX_SUMMARY_TOTAL_ACTIONS
                                {
                                    break 'gsb;
                                }
                                total_bindings += 1;
                                total_actions += g.analog.len();
                                bindings.push((source.to_string(), g.mode.clone(), g.analog.clone()));
                            }
                            for (input_name, acts) in &g.inputs {
                                if bindings.len() >= MAX_SUMMARY_BINDINGS_PER_SET
                                    || total_bindings >= MAX_SUMMARY_TOTAL_BINDINGS
                                    || total_actions.saturating_add(acts.len()) > MAX_SUMMARY_TOTAL_ACTIONS
                                {
                                    break 'gsb;
                                }
                                total_bindings += 1;
                                total_actions += acts.len();
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
    let mut user_vdfs = Vec::new();
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
            user_vdfs.push((text, path));
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

    let mut game_vdfs = Vec::new();
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
            game_vdfs.push((text, path));
        }
    }

    let active_choice = app_binding(app_id);
    let mut active_source = String::new();
    let mut active_path = None;
    let mut active_workshop_id = None;
    let mut active_vdf = None;

    match &active_choice {
        BindingChoice::LocalFile(p) => {
            let local_text = read_bounded_config_file(p);
            if !available.iter().any(|o| o.path.as_ref() == Some(p)) {
                let ct = local_text.as_deref().map(controller_type_in).unwrap_or_default();
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                available.push(BindingOption {
                    choice: BindingChoice::LocalFile(p.clone()),
                    origin: BindingOrigin::CustomFile,
                    controller_type: ct,
                    label: format!("Custom: {name}"),
                    path: Some(p.clone()),
                    workshop_id: None,
                });
            }
            if let Some(t) = local_text
                && is_mappings(&t)
            {
                active_source = format!("Selected file ({})", p.display());
                active_path = Some(p.clone());
                active_vdf = Some(t);
            }
        }
        BindingChoice::Workshop(id) => {
            active_workshop_id = Some(*id);
            active_source = format!("Workshop #{id}");
        }
        BindingChoice::Auto => {
            if let Some((t, p)) = user_vdfs.into_iter().next() {
                active_source = format!("User configuration ({})", p.display());
                active_path = Some(p);
                active_vdf = Some(t);
            } else if let Some((t, p)) = game_vdfs.into_iter().next() {
                active_source = format!("Game files ({})", p.display());
                active_path = Some(p);
                active_vdf = Some(t);
            }
        }
    }

    let (active_controller_type, action_sets) = active_vdf
        .as_deref()
        .map(parse_vdf_summary)
        .unwrap_or_default();

    InputConfigReport {
        app_id,
        active_choice,
        active_source,
        active_path,
        active_workshop_id,
        active_controller_type,
        action_sets,
        available,
        config_dir: cfg_dir,
    }
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

    // Download Workshop / Official VDF if the active config isn't already resolved from disk.
    if report.action_sets.is_empty() {
        let fetched = match report.active_choice {
            BindingChoice::Workshop(id) => workshop_file(conn, app_id, id).await.map(|t| (t, id, format!("Selected Workshop #{id}"))),
            BindingChoice::LocalFile(_) => None,
            BindingChoice::Auto => {
                let mut hit = None;
                for (id, _) in player_workshop_choices(steam_root, account, app_id, true) {
                    if let Some(t) = workshop_file(conn, app_id, id).await {
                        hit = Some((t, id, format!("User Workshop #{id}")));
                        break;
                    }
                }
                if hit.is_none()
                    && let Some((t, id)) = official(conn, app_id).await
                {
                    hit = Some((t, id, format!("Official configuration (Workshop {id})")));
                }
                hit
            }
        };
        if let Some((text, id, src)) = fetched {
            let (ct, sets) = parse_vdf_summary(&text);
            report.active_source = src;
            report.active_workshop_id = Some(id);
            report.active_controller_type = ct;
            report.action_sets = sets;
        }
    }
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
        assert_eq!(report.available.len(), 2);
        assert_eq!(report.active_path.as_ref(), Some(&user_vdf));
        assert_eq!(report.action_sets.len(), 3);

        set_app_binding(620, BindingChoice::LocalFile(game_vdf.clone()));
        let picked = inspect_local(&steam, 42, 620, &game);
        assert_eq!(picked.active_path.as_ref(), Some(&game_vdf));

        set_app_binding(620, BindingChoice::Auto);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn validates_workshop_config_urls_and_bounds_summary() {
        assert!(validate_workshop_config_url("https://steamusercontent-a.akamaihd.net/ugc/123/cfg").is_ok());
        assert!(validate_workshop_config_url("https://images.steamusercontent.com/ugc/1/a").is_ok());
        assert!(validate_workshop_config_url("http://steamusercontent-a.akamaihd.net/ugc/1/a").is_err());
        assert!(validate_workshop_config_url("https://evil.example.com/cfg").is_err());
        assert!(validate_workshop_config_url("https://user:pass@steamusercontent-a.akamaihd.net/ugc/1").is_err());
        assert!(validate_workshop_config_url("https://steamusercontent-a.akamaihd.net.evil.com/x").is_err());

        // Memory amplification guard: repeating the same group across many presets is capped.
        let mut vdf = String::from("\"controller_mappings\" { \"controller_type\" \"controller_xboxone\" ");
        vdf.push_str("\"group\" { \"id\" \"0\" \"mode\" \"four_buttons\" \"inputs\" { ");
        for i in 0..40 {
            vdf.push_str(&format!("\"btn_{i}\" {{ \"activators\" {{ \"Full\" {{ \"bindings\" {{ \"binding\" \"game_action InGame act_{i}, , \" }} }} }} }} "));
        }
        vdf.push_str("} } ");
        for p in 0..100 {
            vdf.push_str(&format!("\"preset\" {{ \"id\" \"{p}\" \"name\" \"Set{p}\" \"group_source_bindings\" {{ \"0\" \"button_diamond active\" \"0\" \"button_diamond active\" }} }} "));
        }
        vdf.push('}');
        let (_, sets) = parse_vdf_summary(&vdf);
        assert!(sets.len() <= MAX_SUMMARY_SETS);
        let total_bindings: usize = sets.iter().map(|s| s.bindings.len()).sum();
        assert!(total_bindings <= MAX_SUMMARY_TOTAL_BINDINGS);
    }
}
