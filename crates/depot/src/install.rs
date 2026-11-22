//! Installing or updating depots into a game folder.
//!
//! - Files that are already correct are skipped.
//! - On updates, chunks that didn't change are copied from the installed file
//!   instead of downloaded (the old manifest says where they are).
//! - Chunks download in parallel; each is verified (SHA-1) before it's written.
//! - New files are written to `*.sirpart` and renamed when complete, so an
//!   interrupted install never leaves a half-written game file.

use crate::cdn::Cdn;
use crate::manifest::{Chunk, FileEntry, Manifest};
use crate::{Error, Result, chunk, crypto};
use futures_util::StreamExt;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

pub struct DepotJob {
    pub depot: u32,
    pub manifest: Manifest,
    pub key: Vec<u8>,
    /// The currently installed version, if any (enables chunk reuse).
    pub old: Option<Manifest>,
}

/// Live progress, shared with the UI.
#[derive(Debug, Default)]
pub struct Progress {
    /// Bytes that must come over the network.
    pub download_total: AtomicU64,
    pub downloaded: AtomicU64,
    /// Bytes that end up on disk (including reused/skipped).
    pub disk_total: AtomicU64,
    pub disk_done: AtomicU64,
    pub files_total: AtomicUsize,
    pub files_done: AtomicUsize,
    pub paused: AtomicBool,
    pub cancel: AtomicBool,
    /// The download ended (done, failed or cancelled): progress updates stop.
    pub ended: AtomicBool,
    /// Bytes actually written to disk (not files found already complete),
    /// for an honest disk speed.
    pub written: AtomicU64,
    /// KB/s, 0 = unlimited.
    pub limit_kbps: AtomicU64,
    /// Parallel chunk downloads.
    pub concurrency: AtomicU32,
    pub phase: std::sync::Mutex<Phase>,
    /// Things worth telling the user that don't stop the download,
    /// e.g. a part of the game this account doesn't include.
    pub notes: std::sync::Mutex<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Preparing,
    Checking,
    Downloading,
    Finishing,
    Done,
}

impl Progress {
    pub fn set_phase(&self, p: Phase) {
        *self.phase.lock().unwrap() = p;
    }
    pub fn phase(&self) -> Phase {
        *self.phase.lock().unwrap()
    }
    pub fn fraction(&self) -> f32 {
        let t = self.disk_total.load(Ordering::Relaxed).max(1);
        self.disk_done.load(Ordering::Relaxed) as f32 / t as f32
    }
}

#[derive(Debug, Default, Clone)]
pub struct Report {
    pub files: usize,
    pub skipped: usize,
    pub reused_bytes: u64,
    pub downloaded_bytes: u64,
    pub removed: usize,
}

/// One unit of work: produce `chunk` for `file` (download or copy).
struct Work {
    file: usize,
    depot: u32,
    key: Arc<crypto::DepotKey>,
    chunk: Chunk,
    /// Where an identical chunk sits in the currently installed file.
    reuse_from: Option<(PathBuf, u64)>,
}

fn sha1_file_with_buf(path: &Path, buf: &mut [u8]) -> Option<Vec<u8>> {
    let mut f = fs::File::open(path).ok()?;
    let mut h = Sha1::new();
    loop {
        let n = f.read(buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize().to_vec())
}

fn part_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".sirpart");
    PathBuf::from(s)
}

fn is_symlink_or_reparse(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

fn is_windows_reserved(seg: &str) -> bool {
    let stem = seg.split('.').next().unwrap_or("");
    let upper = stem.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "CONIN$"
            | "CONOUT$"
            | "COM0"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT0"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        return true;
    }
    if stem.len() > 3
        && stem.is_char_boundary(3)
        && (stem[..3].eq_ignore_ascii_case("COM") || stem[..3].eq_ignore_ascii_case("LPT"))
        && matches!(&stem[3..], "\u{00b9}" | "\u{00b2}" | "\u{00b3}")
    {
        return true;
    }
    false
}

fn is_valid_segment(seg: &str) -> bool {
    if seg.is_empty() || seg == "." || seg == ".." || seg.ends_with('.') || seg.ends_with(' ') {
        return false;
    }
    if seg.contains(['/', '\\', ':', '\0']) || seg.chars().any(|c| (c as u32) < 0x20) {
        return false;
    }
    if seg.to_ascii_lowercase().ends_with(".sirpart") || is_windows_reserved(seg) {
        return false;
    }
    let mut comps = Path::new(seg).components();
    matches!((comps.next(), comps.next()), (Some(Component::Normal(s)), None) if !s.is_empty())
}

/// Validate a single folder name (e.g. `<library>/steamapps/common/<folder>`).
pub fn validate_folder_name(name: &str) -> Result<String> {
    if !is_valid_segment(name) {
        return Err(Error::Format(format!("unsafe folder name: {name:?}")));
    }
    Ok(name.to_string())
}

/// Validate a relative manifest path (`\` normalized to `/`), allowing only
/// non-empty `Component::Normal` segments without reserved names or `.sirpart`.
pub fn validate_rel_path(rel: &str) -> Result<PathBuf> {
    if rel.is_empty() || rel.contains('\0') {
        return Err(Error::Format(format!("unsafe manifest path: {rel:?}")));
    }
    let norm = rel.replace('\\', "/");
    let mut out = PathBuf::new();
    for seg in norm.split('/') {
        if !is_valid_segment(seg) {
            return Err(Error::Format(format!("unsafe manifest path: {rel:?}")));
        }
        out.push(seg);
    }
    if out.as_os_str().is_empty()
        || !out.components().all(|c| matches!(c, Component::Normal(s) if !s.is_empty()))
    {
        return Err(Error::Format(format!("unsafe manifest path: {rel:?}")));
    }
    Ok(out)
}

/// Verify `dir` (if it already exists) is a real directory, not a symlink/reparse point.
pub fn ensure_real_dir(dir: &Path) -> Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(m) if is_symlink_or_reparse(&m) => {
            Err(Error::Format(format!("refusing symlink directory: {}", dir.display())))
        }
        Ok(m) if !m.is_dir() => Err(Error::Format(format!("not a directory: {}", dir.display()))),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Io(e)),
    }
}

fn ensure_not_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if is_symlink_or_reparse(&m) => {
            Err(Error::Format(format!("refusing symlink target: {}", path.display())))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Io(e)),
    }
}

/// Validate `rel` and check via `symlink_metadata` that `dir` and every
/// existing component inside `dir` along `rel` is free of symlinks/reparse points.
pub fn resolve_inside(dir: &Path, rel: &str) -> Result<PathBuf> {
    let rel_path = validate_rel_path(rel)?;
    ensure_real_dir(dir)?;
    let mut cur = dir.to_path_buf();
    let mut comps = rel_path.components().peekable();
    while let Some(c) = comps.next() {
        cur.push(c);
        let is_last = comps.peek().is_none();
        match fs::symlink_metadata(&cur) {
            Ok(m) => {
                if is_symlink_or_reparse(&m) {
                    return Err(Error::Format(format!("refusing symlink path component in {rel:?}")));
                }
                if !is_last && !m.is_dir() {
                    return Err(Error::Format(format!("non-directory ancestor in {rel:?}")));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                for rest in comps {
                    cur.push(rest);
                }
                break;
            }
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(cur)
}

pub async fn install(cdn: Arc<Cdn>, dir: &Path, jobs: Vec<DepotJob>, progress: Arc<Progress>) -> Result<Report> {
    progress.set_phase(Phase::Checking);
    let dir_buf = dir.to_path_buf();
    let progress_plan = progress.clone();

    // Run blocking manifest validation, existing-file SHA-1 checking, and `.sirpart`
    // preallocation on a blocking thread so tokio's async workers stay responsive.
    let (jobs, mut report, files, work) = tokio::task::spawn_blocking(move || -> Result<_> {
        let dir = dir_buf.as_path();
        let mut report = Report::default();
        ensure_real_dir(dir)?;
        for job in &jobs {
            for f in &job.manifest.files {
                validate_rel_path(&f.name)?;
            }
        }
        fs::create_dir_all(dir)?;
        ensure_real_dir(dir)?;

        // A file can be in more than one depot (a language or platform depot
        // replacing a base file). Like Steam, the later depot's copy wins; the
        // earlier one is skipped, so the file is never written (and its
        // temporary file renamed) twice.
        let key_of = |name: &str| name.replace('\\', "/").to_lowercase();
        let mut owner: HashMap<String, usize> = HashMap::new();
        for (i, job) in jobs.iter().enumerate() {
            for f in job.manifest.files.iter().filter(|f| !f.directory) {
                owner.insert(key_of(&f.name), i);
            }
        }

        // Plan: which files need work, and where each chunk comes from.
        let mut files: Vec<(PathBuf, String, FileEntry)> = Vec::new();
        let mut work: Vec<Work> = Vec::new();
        // Reuse one 256 KiB buffer across all file checks instead of mmap/munmap per file.
        let mut hash_buf = vec![0u8; 256 * 1024];
        for (job_index, job) in jobs.iter().enumerate() {
            let key = Arc::new(crypto::DepotKey::new(&job.key)?);
            let mut old_files: HashMap<String, &FileEntry> = HashMap::new();
            // Index of every verified chunk in the installed version, for reuse.
            let mut old_chunks: HashMap<&[u8], (PathBuf, u64)> = HashMap::new();
            for f in job.old.iter().flat_map(|m| &m.files).filter(|f| !f.directory && f.link.is_none()) {
                let Ok(old_path) = resolve_inside(dir, &f.name) else { continue };
                let is_reg = fs::symlink_metadata(&old_path)
                    .map(|m| !is_symlink_or_reparse(&m) && m.is_file())
                    .unwrap_or(false);
                if !is_reg {
                    continue;
                }
                old_files.insert(key_of(&f.name), f);
                for c in &f.chunks {
                    if c.sha.len() == 20 {
                        old_chunks.entry(c.sha.as_slice()).or_insert_with(|| (old_path.clone(), c.offset));
                    }
                }
            }
            for f in &job.manifest.files {
                let path = resolve_inside(dir, &f.name)?;
                if f.directory {
                    fs::create_dir_all(&path)?;
                    ensure_real_dir(&path)?;
                    continue;
                }
                let fkey = key_of(&f.name);
                if f.link.is_some() || owner.get(&fkey) != Some(&job_index) {
                    continue;
                }
                progress_plan.disk_total.fetch_add(f.size, Ordering::Relaxed);
                report.files += 1;
                // Unchanged since the installed version and the size matches: trust it.
                let same_as_old = old_files.get(&fkey).is_some_and(|o| f.sha.len() == 20 && o.sha == f.sha);
                let size_ok = fs::symlink_metadata(&path)
                    .map(|m| !is_symlink_or_reparse(&m) && m.is_file() && m.len() == f.size)
                    .unwrap_or(false);
                if size_ok && (same_as_old || sha1_file_with_buf(&path, &mut hash_buf).is_some_and(|h| f.sha.len() == 20 && h == f.sha)) {
                    report.skipped += 1;
                    progress_plan.disk_done.fetch_add(f.size, Ordering::Relaxed);
                    continue;
                }
                let idx = files.len();
                for c in &f.chunks {
                    let reuse_from = if c.sha.len() == 20 {
                        old_chunks.get(c.sha.as_slice()).cloned()
                    } else {
                        None
                    };
                    if reuse_from.is_none() {
                        progress_plan.download_total.fetch_add(c.compressed as u64, Ordering::Relaxed);
                    }
                    work.push(Work { file: idx, depot: job.depot, key: key.clone(), chunk: c.clone(), reuse_from });
                }
                files.push((path, f.name.clone(), f.clone()));
            }
        }
        progress_plan.files_total.store(files.len(), Ordering::Relaxed);

        // Pre-allocate partial files.
        for (path, rel, f) in &files {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let checked = resolve_inside(dir, rel)?;
            let part = part_path(&checked);
            ensure_not_symlink(&part)?;
            let file = OpenOptions::new().create(true).write(true).truncate(false).open(&part)?;
            file.set_len(f.size)?;
        }
        // Empty files are complete immediately.
        for (path, rel, f) in &files {
            if f.chunks.is_empty() {
                resolve_inside(dir, rel)?;
                finish_file(path)?;
                progress_plan.files_done.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok((jobs, report, files, work))
    })
    .await
    .map_err(|e| Error::Format(format!("install plan task failed: {e}")))??;

    let remaining: Arc<Vec<AtomicUsize>> = Arc::new(
        files
            .iter()
            .map(|(_, _, f)| AtomicUsize::new(if f.chunks.is_empty() { usize::MAX } else { f.chunks.len() }))
            .collect(),
    );

    progress.set_phase(Phase::Downloading);
    let files = Arc::new(files);
    // Keep a small bounded pool of open `.sirpart` handles so multi-thousand-chunk
    // files use positional writes (`pwrite`/`seek_write`) instead of open+close per chunk.
    let open_handles: Arc<Mutex<HashMap<usize, Arc<File>>>> = Arc::new(Mutex::new(HashMap::new()));
    let reused = Arc::new(AtomicU64::new(0));
    let started = Instant::now();
    let dir_buf = dir.to_path_buf();
    let concurrency = progress.concurrency.load(Ordering::Relaxed).clamp(1, 64) as usize;
    let writes = Arc::new(Semaphore::new(concurrency));
    let chunk_stream = futures_util::stream::iter(work.into_iter().map(|w| {
        let (cdn, files, remaining, open_handles, progress, reused, dir_buf) = (
            cdn.clone(),
            files.clone(),
            remaining.clone(),
            open_handles.clone(),
            progress.clone(),
            reused.clone(),
            dir_buf.clone(),
        );
        let writes = writes.clone();
        async move {
            loop {
                if progress.cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                if !progress.paused.load(Ordering::Relaxed) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            let data = match &w.reuse_from {
                Some((src, off)) => {
                    let src = src.clone();
                    let off = *off;
                    let size = w.chunk.size as usize;
                    let sha = w.chunk.sha.clone();
                    let reused_data = tokio::task::spawn_blocking(move || {
                        read_range(&src, off, size).filter(|d| chunk::verify(d, &sha))
                    })
                    .await
                    .ok()
                    .flatten();
                    match reused_data {
                        Some(d) => {
                            reused.fetch_add(d.len() as u64, Ordering::Relaxed);
                            d
                        }
                        None => {
                            progress.download_total.fetch_add(w.chunk.compressed as u64, Ordering::Relaxed);
                            download(&cdn, &w, &progress, started).await?
                        }
                    }
                }
                None => download(&cdn, &w, &progress, started).await?,
            };
            let data_len = data.len() as u64;
            let file_idx = w.file;
            let chunk_offset = w.chunk.offset;
            let permit = writes.acquire_owned().await.expect("write semaphore is never closed");
            tokio::task::spawn_blocking(move || -> Result<()> {
                let _permit = permit;
                if progress.cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let (path, rel, _) = &files[file_idx];
                let handle = {
                    let existing = open_handles.lock().unwrap().get(&file_idx).cloned();
                    match existing {
                        Some(h) => h,
                        None => {
                            resolve_inside(&dir_buf, rel)?;
                            let part = part_path(path);
                            ensure_not_symlink(&part)?;
                            let f = Arc::new(OpenOptions::new().write(true).open(&part)?);
                            let mut map = open_handles.lock().unwrap();
                            if map.len() >= 32 {
                                map.clear();
                            }
                            map.entry(file_idx).or_insert_with(|| f.clone()).clone()
                        }
                    }
                };
                write_file_at(&handle, chunk_offset, &data)?;
                drop(handle);
                progress.disk_done.fetch_add(data_len, Ordering::Relaxed);
                progress.written.fetch_add(data_len, Ordering::Relaxed);
                if remaining[file_idx].fetch_sub(1, Ordering::AcqRel) == 1 {
                    // Release any cached handle BEFORE rename (mandatory on Windows).
                    open_handles.lock().unwrap().remove(&file_idx);
                    resolve_inside(&dir_buf, rel)?;
                    finish_file(path)?;
                    progress.files_done.fetch_add(1, Ordering::Relaxed);
                }
                Ok(())
            })
            .await
            .map_err(|e| Error::Format(format!("chunk write task failed: {e}")))?
        }
    }))
    .buffer_unordered(concurrency);

    let mut stream = chunk_stream;
    let mut failure = None;
    while let Some(result) = stream.next().await {
        if let Err(error) = result {
            failure = Some(error);
            break;
        }
    }
    drop(stream);
    // A dropped spawn_blocking handle does not stop its writes. Wait before
    // returning so cancellation cleanup or a retry cannot race a part-file write.
    if let Some(error) = failure {
        let _drained = writes.acquire_many(concurrency as u32).await.expect("write semaphore is never closed");
        return Err(error);
    }
    open_handles.lock().unwrap().clear();

    // Remove files the new version no longer has and apply executable bits on blocking thread.
    progress.set_phase(Phase::Finishing);
    let dir_finish = dir.to_path_buf();
    let removed = tokio::task::spawn_blocking(move || -> usize {
        let dir = dir_finish.as_path();
        let key_of = |name: &str| name.replace('\\', "/").to_lowercase();
        let mut removed = 0usize;
        for job in &jobs {
            if let Some(old) = &job.old {
                let keep: std::collections::HashSet<String> = job.manifest.files.iter().map(|f| key_of(&f.name)).collect();
                for f in old.files.iter().filter(|f| !f.directory && !keep.contains(&key_of(&f.name))) {
                    if let Ok(path) = resolve_inside(dir, &f.name)
                        && fs::remove_file(path).is_ok()
                    {
                        removed += 1;
                    }
                }
            }
        }
        #[cfg(unix)]
        for job in &jobs {
            for f in job.manifest.files.iter().filter(|f| f.executable && !f.directory && f.link.is_none()) {
                match resolve_inside(dir, &f.name) {
                    Ok(p) => {
                        if let Err(e) = make_executable(&p) {
                            tracing::warn!("couldn't mark {} executable: {e}", f.name);
                        }
                    }
                    Err(e) => tracing::warn!("couldn't mark {} executable: {e}", f.name),
                }
            }
        }
        removed
    })
    .await
    .map_err(|e| Error::Format(format!("install finishing task failed: {e}")))?;
    report.removed = removed;
    report.reused_bytes = reused.load(Ordering::Relaxed);
    report.downloaded_bytes = progress.downloaded.load(Ordering::Relaxed);
    progress.set_phase(Phase::Done);
    Ok(report)
}

async fn download(cdn: &Cdn, w: &Work, progress: &Progress, started: Instant) -> Result<Vec<u8>> {
    let mut last = None;
    for _ in 0..3 {
        let raw = cdn.chunk(w.depot, &w.chunk.sha).await?;
        let len = raw.len() as u64;
        let key = w.key.clone();
        let sha = w.chunk.sha.clone();
        let decoded = tokio::task::spawn_blocking(move || {
            let mut scratch = Vec::new();
            let data = chunk::decode_with_key(&raw, &key, &mut scratch)?;
            if chunk::verify(&data, &sha) {
                Ok(Some(data))
            } else {
                Ok(None)
            }
        })
        .await
        .map_err(|e| Error::Format(format!("chunk decode task failed: {e}")))?;
        match decoded {
            Ok(Some(data)) => {
                let total = progress.downloaded.fetch_add(len, Ordering::Relaxed) + len;
                throttle(progress, total, started).await;
                return Ok(data);
            }
            Ok(None) => last = Some(Error::Format(format!("chunk {} failed verification", chunk::hex(&w.chunk.sha)))),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or(Error::NoServers))
}

/// Keep the average rate under the user's limit.
async fn throttle(progress: &Progress, total: u64, started: Instant) {
    let limit = progress.limit_kbps.load(Ordering::Relaxed);
    if limit == 0 {
        return;
    }
    let should_take = Duration::from_secs_f64(total as f64 / (limit as f64 * 1024.0));
    let took = started.elapsed();
    if should_take > took {
        tokio::time::sleep(should_take - took).await;
    }
}

fn read_range(path: &Path, offset: u64, len: usize) -> Option<Vec<u8>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if is_symlink_or_reparse(&meta) || !meta.is_file() {
        return None;
    }
    let mut f = fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

#[cfg(unix)]
fn write_file_at(file: &File, offset: u64, data: &[u8]) -> Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(data, offset)?;
    Ok(())
}

#[cfg(windows)]
fn write_file_at(file: &File, mut offset: u64, mut data: &[u8]) -> Result<()> {
    use std::os::windows::fs::FileExt;
    while !data.is_empty() {
        let n = file.seek_write(data, offset)?;
        if n == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::WriteZero).into());
        }
        data = &data[n..];
        offset += n as u64;
    }
    Ok(())
}

/// Add the x bits wherever the r bits are (like `chmod +x` with a umask).
#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::symlink_metadata(path)?;
    if is_symlink_or_reparse(&meta) || !meta.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "refusing non-regular file"));
    }
    let mode = meta.permissions().mode();
    let want = mode | ((mode & 0o444) >> 2);
    if want != mode {
        fs::set_permissions(path, fs::Permissions::from_mode(want))?;
    }
    Ok(())
}

fn finish_file(path: &Path) -> Result<()> {
    let part = part_path(path);
    ensure_not_symlink(&part)?;
    ensure_not_symlink(path)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&part, path)?;
    Ok(())
}

#[cfg(all(test, unix))]
mod exec_tests {
    #[test]
    fn marks_programs_executable() {
        use std::os::unix::fs::PermissionsExt;
        let p = std::env::temp_dir().join(format!("sir-exec-{}", std::process::id()));
        std::fs::write(&p, b"x").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        super::make_executable(&p).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o755);
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;

    fn empty_file(name: &str) -> FileEntry {
        FileEntry {
            name: name.into(),
            size: 0,
            sha: Sha1::digest(b"").to_vec(),
            chunks: vec![],
            directory: false,
            executable: true,
            link: None,
        }
    }

    #[tokio::test]
    async fn rejects_traversal_reserved_and_sirpart_manifest_names() {
        let root = std::env::temp_dir().join(format!("sir-aur02-trav-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("game");
        fs::create_dir_all(&dir).unwrap();
        let cdn = Arc::new(Cdn::new(vec![]));

        for bad in [
            "../escaped.txt",
            "sub/../../escaped.txt",
            r"..\escaped.txt",
            "./dot.txt",
            "sub/./dot.txt",
            "/abs_escaped.txt",
            r"\abs_escaped.txt",
            r"C:\evil.dll",
            "C:evil.dll",
            "bad\0name.txt",
            "CON",
            "sub/NUL.txt",
            "com1.bin",
            "game.exe.sirpart",
            "GAME.EXE.SIRPART",
        ] {
            let m = Manifest { depot: 1, gid: 1, files: vec![empty_file(bad)], ..Default::default() };
            let job = DepotJob { depot: 1, manifest: m, key: vec![0; 32], old: None };
            let res = install(cdn.clone(), &dir, vec![job], Arc::new(Progress::default())).await;
            assert!(res.is_err(), "expected {bad:?} to be rejected");
        }
        assert!(!root.join("escaped.txt").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_symlink_escape_and_old_manifest_traversal() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("sir-aur02-sym-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let outside = root.join("outside");
        let dir = root.join("game");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(root.join("victim.txt"), b"keep").unwrap();
        fs::write(outside.join("sym_victim.txt"), b"keep").unwrap();
        symlink(&outside, dir.join("sub")).unwrap();

        let cdn = Arc::new(Cdn::new(vec![]));
        let m = Manifest { depot: 1, gid: 2, files: vec![empty_file("sub/pwned.txt")], ..Default::default() };
        let job = DepotJob { depot: 1, manifest: m, key: vec![0; 32], old: None };
        assert!(install(cdn.clone(), &dir, vec![job], Arc::new(Progress::default())).await.is_err());
        assert!(!outside.join("pwned.txt").exists());

        // Old manifest removal must not delete ../victim.txt or traverse symlink dir/sub.
        let new_m = Manifest { depot: 1, gid: 3, files: vec![empty_file("ok.txt")], ..Default::default() };
        let old_m = Manifest {
            depot: 1,
            gid: 1,
            files: vec![empty_file("../victim.txt"), empty_file("sub/sym_victim.txt")],
            ..Default::default()
        };
        let job2 = DepotJob { depot: 1, manifest: new_m, key: vec![0; 32], old: Some(old_m) };
        install(cdn.clone(), &dir, vec![job2], Arc::new(Progress::default())).await.unwrap();
        assert!(root.join("victim.txt").exists());
        assert!(outside.join("sym_victim.txt").exists());

        // Symlinked .sirpart or top-level install dir must also be refused.
        symlink(outside.join("sym_victim.txt"), dir.join("evil.txt.sirpart")).unwrap();
        let m3 = Manifest { depot: 1, gid: 4, files: vec![empty_file("evil.txt")], ..Default::default() };
        assert!(install(cdn.clone(), &dir, vec![DepotJob { depot: 1, manifest: m3, key: vec![0; 32], old: None }], Arc::new(Progress::default())).await.is_err());
        assert_eq!(fs::read(outside.join("sym_victim.txt")).unwrap(), b"keep");

        let sym_game = root.join("sym_game");
        symlink(&outside, &sym_game).unwrap();
        let m4 = Manifest { depot: 1, gid: 5, files: vec![empty_file("ok.txt")], ..Default::default() };
        assert!(install(cdn, &sym_game, vec![DepotJob { depot: 1, manifest: m4, key: vec![0; 32], old: None }], Arc::new(Progress::default())).await.is_err());
        assert!(!outside.join("ok.txt").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn does_not_skip_existing_file_when_manifest_sha_is_empty() {
        let root = std::env::temp_dir().join(format!("sir-aur02-sha-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("game");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tampered.bin"), b"abcd").unwrap();

        let f = FileEntry {
            name: "tampered.bin".into(),
            size: 4,
            sha: vec![],
            chunks: vec![Chunk { sha: vec![], offset: 0, size: 4, compressed: 4 }],
            directory: false,
            executable: false,
            link: None,
        };
        let m = Manifest { depot: 1, gid: 2, files: vec![f], ..Default::default() };
        let job = DepotJob { depot: 1, manifest: m, key: vec![0; 32], old: None };
        let res = install(Arc::new(Cdn::new(vec![])), &dir, vec![job], Arc::new(Progress::default())).await;
        assert!(res.is_err(), "empty SHA must not skip verification");
        let _ = fs::remove_dir_all(&root);
    }
}
