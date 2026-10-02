use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// Process-level exclusive ownership of a Telegram account's session.
///
/// Keep this value alive for as long as the MTProto client can access its session. The OS releases
/// the advisory lock when the file handle is dropped, including after process termination.
pub struct AccountOwnerLock {
    _file: File,
}

impl AccountOwnerLock {
    /// Opens/creates `path` and acquires an exclusive, non-blocking advisory lock.
    pub fn acquire(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }

        reject_symlink(&path)?;
        let file = open_lock_file(&path)?;
        let opened = file.metadata()?;
        let linked = std::fs::symlink_metadata(&path)?;
        if linked.file_type().is_symlink() || !opened.is_file() || !same_file(&opened, &linked) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Telegram lock path changed while it was being opened",
            ));
        }
        set_private_mode(&file)?;
        file.try_lock()?;
        Ok(Self { _file: file })
    }
}

fn reject_symlink(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing a symbolic-link lock path",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn set_private_mode(file: &File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_mode(_: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn same_file(opened: &std::fs::Metadata, linked: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    opened.dev() == linked.dev() && opened.ino() == linked.ino()
}

#[cfg(not(unix))]
fn same_file(opened: &std::fs::Metadata, linked: &std::fs::Metadata) -> bool {
    opened.file_type().is_file() && linked.file_type().is_file()
}

#[cfg(unix)]
fn open_lock_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn unique_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "telegram-owner-lock-{}-{}.lock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn exclusive_lock_rejects_second_owner_and_releases_on_drop() {
        let path = unique_path();
        let first = AccountOwnerLock::acquire(&path).unwrap();
        assert!(AccountOwnerLock::acquire(&path).is_err());
        drop(first);
        assert!(AccountOwnerLock::acquire(&path).is_ok());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn relative_lock_path_is_supported() {
        let path = PathBuf::from(format!(
            "telegram-owner-relative-{}-{}.lock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let lock = AccountOwnerLock::acquire(&path).unwrap();
        drop(lock);
        let _ = std::fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_lock_paths_are_rejected() {
        use std::os::unix::fs::symlink;
        let target = unique_path();
        let link = target.with_extension("symlink");
        std::fs::write(&target, b"keep").unwrap();
        symlink(&target, &link).unwrap();
        assert!(AccountOwnerLock::acquire(&link).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        let _ = std::fs::remove_file(link);
        let _ = std::fs::remove_file(target);
    }

    #[cfg(unix)]
    #[test]
    fn lock_file_is_private_on_creation() {
        use std::os::unix::fs::PermissionsExt;

        let path = unique_path();
        let _lock = AccountOwnerLock::acquire(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        drop(_lock);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _reopened = AccountOwnerLock::acquire(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_file(path);
    }
}
