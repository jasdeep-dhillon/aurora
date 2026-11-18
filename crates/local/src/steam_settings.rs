//! A few of Steam's own switches that live in `config/config.vdf`, read and
//! changed in place (Steam reads them at start; Valve's engine on the
//! headless host too).
//!
//! Edits touch one line only (or add it) and are checked by parsing the
//! result before it replaces the file. Steam must not be running: it
//! rewrites the file from memory.

use std::path::Path;

const STEAM: &[&str] = &["InstallConfigStore", "Software", "Valve", "Steam"];
const SHADERS: &[&str] = &["InstallConfigStore", "Software", "Valve", "Steam", "ShaderCacheManager"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// Settings > Downloads > Allow downloads during gameplay. With it on,
    /// the engine keeps starting and suspending game updates and shader
    /// downloads while a game runs.
    DownloadsDuringGameplay,
    /// Settings > Downloads > Shader Pre-Caching: download shaders other
    /// players' games compiled for the same GPU and driver.
    ShaderPreCaching,
    /// Settings > Downloads > Allow background processing of Vulkan shaders.
    ShaderBackgroundProcessing,
}

impl Setting {
    fn place(self) -> (&'static [&'static str], &'static str) {
        match self {
            Setting::DownloadsDuringGameplay => (STEAM, "AllowDownloadsDuringGameplay"),
            Setting::ShaderPreCaching => (SHADERS, "EnableShaderManagement"),
            Setting::ShaderBackgroundProcessing => (SHADERS, "EnableShaderBackgroundProcessing"),
        }
    }

    /// Steam's value when the key isn't in the file.
    fn default(self) -> bool {
        !matches!(self, Setting::DownloadsDuringGameplay)
    }
}

fn config(root: &Path) -> std::path::PathBuf {
    root.join("config").join("config.vdf")
}

/// The setting's value, `None` if Steam's config can't be read.
pub fn get(root: &Path, s: Setting) -> Option<bool> {
    let text = std::fs::read_to_string(config(root)).ok()?;
    let (section, key) = s.place();
    Some(match value(&text, section, key) {
        Some(v) => v.trim() != "0",
        None => s.default(),
    })
}

/// Change the setting. `Ok(false)` if it already had that value.
pub fn set(root: &Path, s: Setting, on: bool) -> Result<bool, String> {
    let path = config(root);
    let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!("{}: refusing to overwrite symlink", path.display()));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if get(root, s) == Some(on) {
        return Ok(false);
    }
    let (section, key) = s.place();
    let new = with_value(&text, section, key, if on { "1" } else { "0" })?;
    write_vdf_atomic(&path, new.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

fn write_vdf_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::hash::{BuildHasher, Hasher};
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing parent directory"))?;
    if std::fs::symlink_metadata(dir)?.file_type().is_symlink() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "parent directory is a symlink"));
    }
    let base_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("config.vdf");
    let pid = std::process::id();

    let (mut f, tmp) = 'open: {
        for _ in 0..32 {
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u32(pid);
            h.write_u64(seq);
            let tag = h.finish();
            let candidate = dir.join(format!(".{base_name}.{pid}-{seq}-{tag:016x}.steaminrust-tmp"));
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            match opts.open(&candidate) {
                Ok(file) => break 'open (file, candidate),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "could not create unique temporary file"));
    };

    let write_res = (|| -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    })();
    drop(f);
    if let Err(e) = write_res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

fn value(text: &str, section: &[&str], key: &str) -> Option<String> {
    vdf::parse(text).ok()?.path(section)?.str(key).map(String::from)
}

/// The file with `section/key` set to `val`, or an error if it doesn't have
/// the shape we know how to edit safely.
fn with_value(text: &str, section: &[&str], key: &str, val: &str) -> Result<String, String> {
    vdf::parse(text).map_err(|e| format!("config.vdf: {e}"))?.path(section).ok_or_else(|| format!("config.vdf: no {} section", section.last().unwrap()))?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let depth = section.len() - 1;
    let tabs = |n: usize| "\t".repeat(n);
    // The section's own lines: from its "{" to the matching "}" at its depth.
    let name = format!("{}\"{}\"", tabs(depth), section[depth]);
    let opens: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].trim_end() == name).collect();
    let [open] = opens[..] else { return Err(format!("config.vdf: can't find the {} section", section[depth])) };
    if lines.get(open + 1).map(|l| l.trim_end()) != Some(&format!("{}{{", tabs(depth))) {
        return Err("config.vdf: unexpected layout".into());
    }
    let close = (open + 2..lines.len()).find(|&i| lines[i].trim_end() == format!("{}}}", tabs(depth))).ok_or("config.vdf: unexpected layout")?;
    let quoted = format!("{}\"{key}\"", tabs(depth + 1));
    let keyed: Vec<usize> = (open + 2..close).filter(|&i| lines[i].starts_with(&quoted)).collect();
    let line = |eol: &str| format!("{quoted}\t\t\"{val}\"{eol}");
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    match keyed[..] {
        [i] => out[i] = line(eol(lines[i])),
        [] => out.insert(open + 2, line(eol(lines[open + 1]))),
        _ => return Err(format!("config.vdf: {key} appears more than once")),
    }
    let new = out.concat();
    // Never write something Steam would read differently from what we meant.
    if value(&new, section, key).as_deref() != Some(val) {
        return Err("config.vdf: edit didn't take".into());
    }
    Ok(new)
}

fn eol(line: &str) -> &'static str {
    if line.ends_with("\r\n") { "\r\n" } else if line.ends_with('\n') { "\n" } else { "" }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "AllowDownloadsDuringGameplay";
    const FILE: &str = "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"ShaderCacheManager\"\n\t\t\t\t{\n\t\t\t\t\t\"App\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"EnableShaderBackgroundProcessing\"\t\t\"7\"\n\t\t\t\t\t}\n\t\t\t\t\t\"EnableShaderBackgroundProcessing\"\t\t\"1\"\n\t\t\t\t}\n\t\t\t\t\"AllowDownloadsDuringGameplay\"\t\t\"1\"\n\t\t\t\t\"CurrentCellID\"\t\t\"14\"\n\t\t\t}\n\t\t}\n\t}\n}\n";

    #[test]
    fn changes_one_line_in_place() {
        let new = with_value(FILE, STEAM, KEY, "0").unwrap();
        assert_eq!(new, FILE.replace("Gameplay\"\t\t\"1\"", "Gameplay\"\t\t\"0\""));
    }

    #[test]
    fn adds_a_missing_key_to_its_section() {
        let absent = FILE.replace("\t\t\t\t\"AllowDownloadsDuringGameplay\"\t\t\"1\"\n", "");
        let new = with_value(&absent, STEAM, KEY, "0").unwrap();
        assert_eq!(value(&new, STEAM, KEY).as_deref(), Some("0"));
        assert_eq!(new.lines().count(), absent.lines().count() + 1);
        assert_eq!(value(&new, STEAM, "CurrentCellID").as_deref(), Some("14"));
        let shaders = with_value(FILE, SHADERS, "EnableShaderManagement", "0").unwrap();
        assert_eq!(value(&shaders, SHADERS, "EnableShaderManagement").as_deref(), Some("0"));
    }

    #[test]
    fn edits_the_direct_child_not_a_nested_one() {
        let new = with_value(FILE, SHADERS, "EnableShaderBackgroundProcessing", "0").unwrap();
        assert!(new.contains("\t\t\t\t\t\t\"EnableShaderBackgroundProcessing\"\t\t\"7\""));
        assert_eq!(value(&new, SHADERS, "EnableShaderBackgroundProcessing").as_deref(), Some("0"));
    }

    #[test]
    fn keeps_crlf() {
        let crlf = FILE.replace('\n', "\r\n");
        let new = with_value(&crlf, STEAM, KEY, "0").unwrap();
        assert_eq!(new, crlf.replace("Gameplay\"\t\t\"1\"", "Gameplay\"\t\t\"0\""));
    }

    #[test]
    fn refuses_odd_files() {
        let twice = FILE.replace("\t\t\t\t\"CurrentCellID\"", "\t\t\t\t\"AllowDownloadsDuringGameplay\"\t\t\"1\"\n\t\t\t\t\"CurrentCellID\"");
        assert!(with_value(&twice, STEAM, KEY, "0").is_err());
        assert!(with_value("\"x\"\n{\n}\n", STEAM, KEY, "0").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn set_uses_excl_temp_and_never_follows_planted_tmp_symlink() {
        let dir = std::env::temp_dir().join(format!("sir-steam-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfg_dir = dir.join("config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let vdf_path = cfg_dir.join("config.vdf");
        std::fs::write(&vdf_path, FILE).unwrap();

        let victim = dir.join("victim.txt");
        std::fs::write(&victim, "keep-me").unwrap();
        // Plant the old predictable `.vdf.steaminrust-tmp` symlink!
        let planted = vdf_path.with_extension("vdf.steaminrust-tmp");
        std::os::unix::fs::symlink(&victim, &planted).unwrap();

        assert_eq!(set(&dir, Setting::DownloadsDuringGameplay, false), Ok(true));
        assert_eq!(get(&dir, Setting::DownloadsDuringGameplay), Some(false));
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep-me", "planted temp symlink was followed!");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
