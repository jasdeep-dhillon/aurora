//! Steam's shader cache for games we start (Linux).
//!
//! Steam keeps a per-game folder, `<library>/steamapps/shadercache/<app>`:
//! Vulkan pipelines in Fossilize databases (`fozpipelinesv6/`, downloaded by
//! shader pre-caching or recorded while playing) and the GPU driver's own
//! cache built from them (`nvidiav1/` for NVIDIA, Mesa's files at the top).
//! Games only use it when started with the environment Steam gives them,
//! which [`apply_env`] reproduces.
//!
//! [`compile`] is Steam's "Processing Vulkan shaders" before launch: it runs
//! Valve's own `fossilize_replay` (from the user's Steam install) over the
//! databases with the same environment, so the driver stores the compiled
//! pipelines where the game will look for them.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

/// Compile a game's shaders before it starts (Settings > Downloads). On by
/// default, like Steam.
static PRECOMPILE: AtomicBool = AtomicBool::new(true);

pub fn set_precompile(on: bool) {
    PRECOMPILE.store(on, Ordering::Relaxed);
}

pub fn precompile() -> bool {
    PRECOMPILE.load(Ordering::Relaxed)
}

/// `<library>/steamapps/shadercache/<app>` for a game installed in
/// `<library>/steamapps/common/<dir>`.
pub fn cache_dir(install_dir: &Path, app_id: model::AppId) -> Option<PathBuf> {
    let steamapps = install_dir.parent()?.parent()?;
    Some(steamapps.join("shadercache").join(app_id.to_string()))
}

/// The variables Steam sets so the GPU driver reads and writes the game's
/// shader cache (names from Valve's engine; values as Steam uses them).
fn env_vars(cache: &Path) -> Vec<(&'static str, String)> {
    let dir = cache.display().to_string();
    vec![
        ("STEAM_COMPAT_SHADER_PATH", dir.clone()),
        // NVIDIA: the driver's cache lives in nvidiav1/GLCache/<driver>/<gpu>/.
        ("__GL_SHADER_DISK_CACHE", "1".into()),
        ("__GL_SHADER_DISK_CACHE_PATH", cache.join("nvidiav1").display().to_string()),
        ("__GL_SHADER_DISK_CACHE_APP_NAME", "steamapp_shader_cache".into()),
        ("__GL_SHADER_DISK_CACHE_READ_ONLY_APP_NAME", "steam_shader_cache;steamapp_merged_shader_cache".into()),
        ("__GL_SHADER_DISK_CACHE_SKIP_CLEANUP", "1".into()),
        // Mesa (AMD, Intel).
        ("MESA_SHADER_CACHE_DIR", dir.clone()),
        ("MESA_GLSL_CACHE_DIR", dir.clone()),
        ("MESA_DISK_CACHE_SINGLE_FILE", "1".into()),
        ("MESA_DISK_CACHE_READ_ONLY_FOZ_DBS", "steam_cache,steam_precompiled".into()),
        ("DXVK_STATE_CACHE_PATH", cache.join("DXVK_state_cache").display().to_string()),
    ]
}

/// Point a game at its shader cache, like Steam does. Variables the player
/// set themselves are kept.
pub fn apply_env(cmd: &mut Command, install_dir: &Path, app_id: model::AppId) {
    let Some(cache) = cache_dir(install_dir, app_id) else { return };
    // The NVIDIA driver only writes into folders that exist.
    let _ = std::fs::create_dir_all(cache.join("nvidiav1"));
    let _ = std::fs::create_dir_all(cache.join("DXVK_state_cache"));
    for (k, v) in env_vars(&cache) {
        if std::env::var_os(k).is_none() {
            cmd.env(k, v);
        }
    }
}

/// Shaders a game has that weren't compiled yet for this GPU driver.
pub struct Pending {
    app_id: model::AppId,
    cache: PathBuf,
    dbs: Vec<PathBuf>,
    stamp: String,
}

/// The pipeline databases to replay: all of `fozpipelinesv6/`, except
/// Fossilize's own bookkeeping (replay caches, whitelists).
fn databases(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            databases(&p, out);
        } else if name.ends_with(".foz") && !name.starts_with("replay_cache") && !name.contains("whitelist") && e.metadata().is_ok_and(|m| m.len() > 0) {
            out.push(p);
        }
    }
}

/// What the compiled cache depends on: the databases, the driver and
/// Valve's replayer. A change in any of them means compiling again.
fn stamp(dbs: &[PathBuf], replayer: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let meta = |p: &Path| std::fs::metadata(p).ok().map(|m| (m.len(), m.modified().ok()));
    for p in dbs.iter().map(PathBuf::as_path).chain([replayer]) {
        p.hash(&mut h);
        meta(p).hash(&mut h);
    }
    driver().hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The GPU driver in use: NVIDIA's module version, else the Vulkan drivers
/// installed (Mesa updates change their files).
fn driver() -> String {
    if let Ok(v) = std::fs::read_to_string("/sys/module/nvidia/version") {
        return format!("nvidia {}", v.trim());
    }
    let mut icds: Vec<String> = ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"]
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .filter_map(|e| {
            let lib = std::fs::read_to_string(e.path()).ok()?;
            let lib = lib.split("\"library_path\"").nth(1)?.split('"').nth(1)?.to_string();
            let m = std::fs::metadata(&lib).ok()?;
            Some(format!("{lib}:{}:{:?}", m.len(), m.modified().ok()))
        })
        .collect();
    icds.sort();
    icds.join(";")
}

fn done_file(app_id: model::AppId) -> PathBuf {
    crate::store::config_dir().join("shaders").join(format!("{app_id}.done"))
}

fn replayer() -> Option<PathBuf> {
    Some(local::find_steam_root()?.join("ubuntu12_64").join("fossilize_replay")).filter(|p| p.is_file())
}

/// The game's shaders, if there are some to compile before it starts.
pub fn pending(install_dir: &Path, app_id: model::AppId) -> Option<Pending> {
    if !precompile() {
        return None;
    }
    let cache = cache_dir(install_dir, app_id)?;
    let mut dbs = Vec::new();
    databases(&cache.join("fozpipelinesv6"), &mut dbs);
    if dbs.is_empty() {
        return None;
    }
    dbs.sort();
    let stamp = stamp(&dbs, &replayer()?);
    if std::fs::read_to_string(done_file(app_id)).is_ok_and(|s| s.trim() == stamp) {
        return None;
    }
    Some(Pending { app_id, cache, dbs, stamp })
}

/// `Overall 1234 / 5678` from Fossilize's progress report.
fn overall(line: &str) -> Option<(u32, u32)> {
    let rest = line.split("Overall").nth(1)?;
    let (done, total) = rest.split_once('/')?;
    Some((done.trim().parse().ok()?, total.trim().parse().ok()?))
}

/// Compile the shaders, reporting `(done, total)` as it goes. Stops early
/// (without remembering it as done) when `cancel` is set. Pipelines already
/// in the driver's cache go fast, so a skipped run picks up where it was.
pub fn compile(p: &Pending, cancel: &AtomicBool, mut progress: impl FnMut(u32, u32)) -> Result<(), String> {
    let replayer = replayer().ok_or("Steam's fossilize_replay is missing")?;
    let _ = std::fs::create_dir_all(p.cache.join("nvidiav1"));
    // Leave a core for the rest of the desktop.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().saturating_sub(1).max(1));
    let mut cmd = Command::new(&replayer);
    cmd.arg("--progress").arg("--num-threads").arg(threads.to_string()).args(&p.dbs);
    cmd.envs(env_vars(&p.cache)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    tracing::info!("compiling shaders for {} ({} databases, {threads} threads)", p.app_id, p.dbs.len());
    let mut child = cmd.spawn().map_err(|e| format!("couldn't start fossilize_replay: {e}"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    let stderr = child.stderr.take().ok_or("no stderr")?;
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Some(o) = overall(&line) {
                let _ = tx.send(o);
            }
        }
    });
    let status = loop {
        while let Ok((done, total)) = rx.try_recv() {
            progress(done, total);
        }
        if cancel.load(Ordering::Relaxed) {
            crate::launch::kill_tree(child.id());
            let _ = child.wait();
            tracing::info!("shader compile for {} skipped", p.app_id);
            return Ok(());
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
            Err(e) => return Err(e.to_string()),
        }
    };
    // Even a run that crashed in the driver is remembered: running it again
    // would only crash again. The game compiles what's missing itself.
    if !status.success() {
        tracing::warn!("fossilize_replay for {} ended with {status}", p.app_id);
    }
    let done = done_file(p.app_id);
    if let Some(dir) = done.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(done, &p.stamp);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fossilize_progress() {
        assert_eq!(overall("Fossilize INFO:    Overall 120 / 4096"), Some((120, 4096)));
        assert_eq!(overall("Fossilize INFO:    Compile graphics 186 / 207, skipped 21"), None);
    }

    #[test]
    fn finds_the_cache_beside_the_library() {
        let d = cache_dir(Path::new("/lib/steamapps/common/Game"), 42).unwrap();
        assert_eq!(d, Path::new("/lib/steamapps/shadercache/42"));
    }
}
