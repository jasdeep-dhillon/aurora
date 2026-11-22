use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use vdf::controller::MAX_BYTES;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(super) fn is_link(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

fn bounded(file: File) -> io::Result<String> {
    let meta = file.metadata()?;
    if !meta.is_file() || is_link(&meta) || meta.len() > MAX_BYTES as u64 {
        return Err(invalid("expected a regular controller layout of at most 4 MiB"));
    }
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Err(invalid("controller layout exceeds 4 MiB"));
    }
    String::from_utf8(bytes).map_err(|_| invalid("controller layout is not UTF-8"))
}

pub(super) fn read_file(path: &Path) -> io::Result<String> {
    let mut open = OpenOptions::new();
    open.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        open.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    bounded(open.open(path)?)
}

/// Hold a directory handle throughout the read or commit. The roots are the
/// user's configured Steam/data directories. All descendants are untrusted.
pub(super) struct Directory {
    pub path: PathBuf,
    #[cfg(unix)]
    fd: std::os::fd::OwnedFd,
    #[cfg(windows)]
    _locks: Vec<File>,
}

impl Directory {
    pub fn open(root: &Path, parts: &[&str], create: bool) -> io::Result<Self> {
        if create {
            fs::create_dir_all(root)?;
        }
        let mut path = root.canonicalize()?;
        #[cfg(unix)]
        use rustix::fs::{Mode, OFlags, mkdirat, open, openat};
        #[cfg(unix)]
        let flags = OFlags::DIRECTORY | OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        #[cfg(unix)]
        let mut fd = open(&path, flags, Mode::empty())?;
        #[cfg(windows)]
        let mut locks = vec![lock_directory(&path)?];
        for part in parts {
            path.push(part);
            #[cfg(unix)]
            {
                if create {
                    match mkdirat(&fd, *part, Mode::from_bits_truncate(0o700)) {
                        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                fd = openat(&fd, *part, flags, Mode::empty())?;
            }
            #[cfg(not(unix))]
            {
                if create {
                    match fs::create_dir(&path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(e) => return Err(e),
                    }
                }
                #[cfg(windows)]
                locks.push(lock_directory(&path)?);
            }
        }
        Ok(Self {
            path,
            #[cfg(unix)]
            fd,
            #[cfg(windows)]
            _locks: locks,
        })
    }

    pub fn read(&self, name: &str) -> io::Result<String> {
        #[cfg(unix)]
        {
            use rustix::fs::{Mode, OFlags, openat};
            let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
            bounded(File::from(openat(&self.fd, name, flags, Mode::empty())?))
        }
        #[cfg(not(unix))]
        read_file(&self.path.join(name))
    }

    pub fn commit(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > MAX_BYTES {
            return Err(invalid("controller layout exceeds 4 MiB"));
        }
        let temp = format!(".{}.tmp", crate::ipc::os_random_hex(16));
        #[cfg(unix)]
        {
            use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, statat, unlinkat};
            let mut f = File::from(openat(
                &self.fd, temp.as_str(), OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )?);
            let result = (|| {
                f.write_all(bytes)?;
                f.sync_all()?;
                match statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(meta) if rustix::fs::FileType::from_raw_mode(meta.st_mode) == rustix::fs::FileType::RegularFile => {}
                    Err(rustix::io::Errno::NOENT) => {}
                    _ => return Err(invalid("unsafe controller layout destination")),
                }
                renameat(&self.fd, temp.as_str(), &self.fd, name)?;
                Ok(())
            })();
            if result.is_err() {
                let _ = unlinkat(&self.fd, temp.as_str(), AtFlags::empty());
            }
            result
        }
        #[cfg(not(unix))]
        {
            let temp = self.path.join(temp);
            let mut f = OpenOptions::new().write(true).create_new(true).open(&temp)?;
            let destination = self.path.join(name);
            let result = (|| {
                f.write_all(bytes)?;
                f.sync_all()?;
                drop(f);
                match fs::symlink_metadata(&destination) {
                    Ok(meta) if !is_link(&meta) && meta.is_file() => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    _ => return Err(invalid("unsafe controller layout destination")),
                }
                fs::rename(&temp, destination)
            })();
            if result.is_err() {
                let _ = fs::remove_file(temp);
            }
            result
        }
    }
}

#[cfg(windows)]
fn lock_directory(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = OpenOptions::new().access_mode(0x80).share_mode(3)
        .custom_flags(0x02000000 | 0x00200000).open(path)?; // BACKUP_SEMANTICS | OPEN_REPARSE_POINT
    let meta = dir.metadata()?;
    if !meta.is_dir() || is_link(&meta) {
        return Err(invalid("unsafe controller layout directory"));
    }
    Ok(dir)
}
