//! Safe, atomic theme package importer for `.zip` archives and local folders.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use crate::Error;
use crate::manifest::{ThemeManifest, ThemeMeta, validate_id};
use crate::tree::LayoutTree;

/// Maximum total uncompressed size allowed for a theme package (50 MiB).
pub const MAX_UNCOMPRESSED_BYTES: u64 = 50 * 1024 * 1024;
/// Maximum number of files allowed in a theme package.
pub const MAX_FILE_COUNT: usize = 500;

/// Safely validate (`theme.toml` + `layout.toml` if present) and atomically
/// install a `.zip` theme package into `themes_dir`.
///
/// Supports both flat archives (`theme.toml` at archive root) and GitHub-style
/// wrapped archives (`my-theme-main/theme.toml` inside a single top-level dir).
pub fn install_zip(zip_path: &Path, themes_dir: &Path) -> Result<ThemeMeta, Error> {
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    if archive.len() > MAX_FILE_COUNT * 2 {
        return Err(Error::PackageLimit(format!(
            "archive has too many entries (limit {MAX_FILE_COUNT} files)"
        )));
    }

    let mut file_count = 0usize;
    let mut declared_bytes = 0u64;
    let mut top_components: HashSet<String> = HashSet::new();
    let mut has_root_manifest = false;
    let mut entry_paths: Vec<(usize, PathBuf, bool)> = Vec::with_capacity(archive.len());

    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        if entry.is_symlink() || is_unix_symlink(&entry) {
            continue;
        }
        let enclosed = entry
            .enclosed_name()
            .ok_or_else(|| Error::UnsafePath(entry.name().to_string()))?;

        for comp in enclosed.components() {
            match comp {
                Component::Normal(_) => {}
                _ => return Err(Error::UnsafePath(entry.name().to_string())),
            }
        }

        if let Some(Component::Normal(first)) = enclosed.components().next() {
            top_components.insert(first.to_string_lossy().into_owned());
        }

        let is_dir = entry.is_dir();
        if !is_dir {
            file_count += 1;
            if file_count > MAX_FILE_COUNT {
                return Err(Error::PackageLimit(format!(
                    "package exceeds maximum of {MAX_FILE_COUNT} files"
                )));
            }
            declared_bytes = declared_bytes.saturating_add(entry.size());
            if declared_bytes > MAX_UNCOMPRESSED_BYTES {
                return Err(Error::PackageLimit(format!(
                    "package exceeds maximum uncompressed size of {} MiB",
                    MAX_UNCOMPRESSED_BYTES / (1024 * 1024)
                )));
            }
            if enclosed == Path::new("theme.toml") {
                has_root_manifest = true;
            }
        }
        entry_paths.push((i, enclosed, is_dir));
    }

    let strip_prefix: Option<PathBuf> = if has_root_manifest {
        None
    } else if top_components.len() == 1 {
        let top = top_components.into_iter().next().unwrap();
        let candidate = Path::new(&top).join("theme.toml");
        if entry_paths.iter().any(|(_, p, is_dir)| !is_dir && p == &candidate) {
            Some(PathBuf::from(top))
        } else {
            return Err(Error::MissingManifest);
        }
    } else {
        return Err(Error::MissingManifest);
    };

    let rel_in_zip = |name: &str| -> PathBuf {
        match &strip_prefix {
            Some(prefix) => prefix.join(name),
            None => PathBuf::from(name),
        }
    };

    // Validate `theme.toml` before extracting anything.
    let manifest_rel = rel_in_zip("theme.toml");
    let manifest_idx = entry_paths
        .iter()
        .find(|(_, p, is_dir)| !is_dir && p == &manifest_rel)
        .map(|(idx, _, _)| *idx)
        .ok_or(Error::MissingManifest)?;

    let manifest_text = read_zip_string(&mut archive, manifest_idx)?;
    let manifest = ThemeManifest::parse(&manifest_text)?;

    // Validate `layout.toml` (and explicit `layout_file` if specified) before committing.
    let mut layout_files_to_check: HashSet<String> = HashSet::new();
    layout_files_to_check.insert("layout.toml".into());
    if let Some(lf) = &manifest.layout_file {
        layout_files_to_check.insert(lf.clone());
    }
    for lf in layout_files_to_check {
        let layout_rel = rel_in_zip(&lf);
        if let Some((idx, _, _)) = entry_paths
            .iter()
            .find(|(_, p, is_dir)| !is_dir && p == &layout_rel)
        {
            let layout_text = read_zip_string(&mut archive, *idx)?;
            LayoutTree::parse(&layout_text).map_err(Error::InvalidLayout)?;
        }
    }

    // Atomic extraction into `themes_dir/.tmp-<id>`.
    fs::create_dir_all(themes_dir)?;
    let tmp_dir = themes_dir.join(format!(".tmp-{}", manifest.meta.id));
    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir)?;
    }
    fs::create_dir_all(&tmp_dir)?;

    let extract_res = (|| -> Result<(), Error> {
        let mut total_written = 0u64;
        for (idx, enclosed, is_dir) in &entry_paths {
            let rel = match &strip_prefix {
                Some(prefix) => match enclosed.strip_prefix(prefix) {
                    Ok(r) => r,
                    Err(_) => continue,
                },
                None => enclosed.as_path(),
            };
            if rel.as_os_str().is_empty() {
                continue;
            }
            let out_path = tmp_dir.join(rel);
            if *is_dir {
                fs::create_dir_all(&out_path)?;
                continue;
            }
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut entry = archive.by_index(*idx)?;
            let mut out = File::create(&out_path)?;
            let remaining = MAX_UNCOMPRESSED_BYTES.saturating_sub(total_written);
            let copied = io::copy(&mut entry.by_ref().take(remaining + 1), &mut out)?;
            total_written = total_written.saturating_add(copied);
            if total_written > MAX_UNCOMPRESSED_BYTES {
                return Err(Error::PackageLimit(format!(
                    "package exceeds maximum uncompressed size of {} MiB",
                    MAX_UNCOMPRESSED_BYTES / (1024 * 1024)
                )));
            }
        }
        Ok(())
    })();

    if let Err(e) = extract_res {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(e);
    }

    commit_tmp_dir(&tmp_dir, &themes_dir.join(&manifest.meta.id))?;
    Ok(manifest.meta)
}

/// Validate a local theme folder (`theme.toml` + `layout.toml` if present) and
/// copy it atomically into `themes_dir/<id>`.
pub fn install_folder(src_dir: &Path, themes_dir: &Path) -> Result<ThemeMeta, Error> {
    let manifest_path = src_dir.join("theme.toml");
    if !manifest_path.is_file() {
        return Err(Error::MissingManifest);
    }
    let manifest_text = fs::read_to_string(&manifest_path)?;
    let manifest = ThemeManifest::parse(&manifest_text)?;

    let layout_path = src_dir.join(manifest.layout_file.as_deref().unwrap_or("layout.toml"));
    if layout_path.is_file() {
        let layout_text = fs::read_to_string(&layout_path)?;
        LayoutTree::parse(&layout_text).map_err(Error::InvalidLayout)?;
    }

    fs::create_dir_all(themes_dir)?;
    let tmp_dir = themes_dir.join(format!(".tmp-{}", manifest.meta.id));
    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir)?;
    }
    fs::create_dir_all(&tmp_dir)?;

    let mut file_count = 0usize;
    let mut total_bytes = 0u64;
    if let Err(e) = copy_dir_safe(src_dir, &tmp_dir, &mut file_count, &mut total_bytes) {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(e);
    }

    commit_tmp_dir(&tmp_dir, &themes_dir.join(&manifest.meta.id))?;
    Ok(manifest.meta)
}

/// Remove an installed theme folder by validated id.
pub fn uninstall_theme(themes_dir: &Path, id: &str) -> Result<(), Error> {
    validate_id(id)?;
    let dir = themes_dir.join(id);
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

fn read_zip_string(archive: &mut zip::ZipArchive<File>, idx: usize) -> Result<String, Error> {
    let mut entry = archive.by_index(idx)?;
    let mut buf = String::new();
    entry
        .by_ref()
        .take(2 * 1024 * 1024)
        .read_to_string(&mut buf)?;
    Ok(buf)
}

fn commit_tmp_dir(tmp_dir: &Path, final_dir: &Path) -> Result<(), Error> {
    if final_dir.exists() {
        fs::remove_dir_all(final_dir)?;
    }
    fs::rename(tmp_dir, final_dir)?;
    Ok(())
}

fn is_unix_symlink(entry: &zip::read::ZipFile<'_, File>) -> bool {
    entry.unix_mode().is_some_and(|m| (m & 0o170000) == 0o120000)
}

fn copy_dir_safe(
    src: &Path,
    dst: &Path,
    file_count: &mut usize,
    total_bytes: &mut u64,
) -> Result<(), Error> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            fs::create_dir_all(&target)?;
            copy_dir_safe(&entry.path(), &target, file_count, total_bytes)?;
        } else if file_type.is_file() {
            *file_count += 1;
            if *file_count > MAX_FILE_COUNT {
                return Err(Error::PackageLimit(format!(
                    "package exceeds maximum of {MAX_FILE_COUNT} files"
                )));
            }
            *total_bytes = total_bytes.saturating_add(meta.len());
            if *total_bytes > MAX_UNCOMPRESSED_BYTES {
                return Err(Error::PackageLimit(format!(
                    "package exceeds maximum uncompressed size of {} MiB",
                    MAX_UNCOMPRESSED_BYTES / (1024 * 1024)
                )));
            }
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
