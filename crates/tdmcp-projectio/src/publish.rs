//! Safe filesystem publication primitives.
//!
//! The public operation is an atomic, fail-closed no-replace rename on the
//! supported release platforms. It never falls back to check-then-rename.

use std::path::Path;

/// Stable identity of one filesystem entry, without following a final symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity(IdentityInner);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdentityInner {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows { volume: u64, index: u64 },
}

/// Read the identity of `path` without following its final symlink.
///
/// # Errors
/// Returns `NotFound` when the entry is absent and `Unsupported` when the
/// platform cannot expose a stable identity required for race reconciliation.
pub fn identity(path: &Path) -> std::io::Result<FileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(path)?;
        Ok(FileIdentity(IdentityInner::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        }))
    }

    #[cfg(windows)]
    {
        windows_identity(path)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!(
                "filesystem entry identity unsupported on this platform: {}",
                path.display()
            ),
        ))
    }
}

#[cfg(windows)]
fn windows_identity(path: &Path) -> std::io::Result<FileIdentity> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    // Documented Win32 CreateFile flags. OPEN_REPARSE_POINT is essential: the
    // identity must describe the final symlink/reparse point, not its target.
    // https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-createfilew
    // https://learn.microsoft.com/windows/win32/fileio/file-attribute-constants
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;

    reject_nul(path)?;
    let file = OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(path)?;
    let information = winapi_util::file::information(&file)?;
    Ok(FileIdentity(IdentityInner::Windows {
        volume: information.volume_serial_number(),
        index: information.file_index(),
    }))
}

/// Atomically rename `from` to `to` only if `to` does not exist.
///
/// This calls `renamore::rename_exclusive` directly. Unsupported platforms or
/// filesystems fail closed; there is deliberately no check-then-rename fallback.
/// Embedded NUL path components are rejected before the platform helper,
/// including on Windows where the upstream UTF-16 conversion does not do so.
pub fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    reject_nul(from)?;
    reject_nul(to)?;
    renamore::rename_exclusive(from, to)
}

/// Atomically rename `from` onto `to`, replacing an existing `to`.
///
/// This exists for the *own* bookkeeping files a caller already owns, such as a
/// durable journal that is updated in place. It is deliberately NOT a fallback
/// for user data: replacing a destination needs a different safety argument than
/// creating one, so the two operations stay separate calls and no
/// no-replace path silently degrades into this one.
///
/// `from` must be fully written and fsynced by the caller first, so a crash
/// leaves either the previous record or the new one, never a truncated file.
pub fn rename_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    reject_nul(from)?;
    reject_nul(to)?;
    // `renamore` only offers the no-replace operation; the replacing rename is
    // plain `rename(2)` on Unix and `MoveFileExW` with
    // `MOVEFILE_REPLACE_EXISTING` on Windows, which is what `std::fs::rename`
    // calls. Both are atomic within one filesystem.
    std::fs::rename(from, to)
}

fn reject_nul(path: &Path) -> std::io::Result<()> {
    if contains_nul(path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("path contains an embedded NUL: {}", path.display()),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn contains_nul(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().contains(&0)
}

#[cfg(windows)]
fn contains_nul(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().any(|unit| unit == 0)
}

#[cfg(not(any(unix, windows)))]
fn contains_nul(path: &Path) -> bool {
    path.as_os_str().to_string_lossy().contains('\0')
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "unit tests")]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn replacing_rename_overwrites_its_own_previous_content() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("from");
        let to = temp.path().join("to");
        fs::write(&from, b"new").unwrap();
        fs::write(&to, b"previous").unwrap();

        rename_replace(&from, &to).unwrap();

        assert!(!from.exists());
        assert_eq!(fs::read(to).unwrap(), b"new");
    }

    #[test]
    fn replacing_rename_does_not_weaken_the_no_replace_guarantee() {
        // The two primitives stay distinct: a no-replace call that fails must
        // never quietly fall through to replacing the destination.
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("from");
        let to = temp.path().join("to");
        fs::write(&from, b"new").unwrap();
        fs::write(&to, b"concurrent").unwrap();

        let error = rename_noreplace(&from, &to).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&from).unwrap(), b"new");
        assert_eq!(fs::read(&to).unwrap(), b"concurrent");
    }

    #[cfg(unix)]
    #[test]
    fn replacing_rename_rejects_an_embedded_nul_before_the_platform_helper() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let bad = Path::new(OsStr::from_bytes(b"bad\0path"));
        let error = rename_replace(bad, Path::new("unused")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn exclusive_rename_moves_file_when_destination_is_absent() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("from");
        let to = temp.path().join("to");
        fs::write(&from, b"new").unwrap();

        rename_noreplace(&from, &to).unwrap();

        assert!(!from.exists());
        assert_eq!(fs::read(to).unwrap(), b"new");
    }

    #[test]
    fn exclusive_rename_never_replaces_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("from");
        let to = temp.path().join("to");
        fs::write(&from, b"new").unwrap();
        fs::write(&to, b"concurrent").unwrap();

        let error = rename_noreplace(&from, &to).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(from).unwrap(), b"new");
        assert_eq!(fs::read(to).unwrap(), b"concurrent");
    }

    #[test]
    fn exclusive_rename_never_replaces_existing_empty_directory() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("from");
        let to = temp.path().join("to");
        fs::create_dir(&from).unwrap();
        fs::write(from.join("new.marker"), b"new").unwrap();
        fs::create_dir(&to).unwrap();

        let error = rename_noreplace(&from, &to).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(from.join("new.marker").is_file());
        assert!(to.is_dir());
        assert!(fs::read_dir(to).unwrap().next().is_none());
    }

    #[test]
    fn identity_tracks_entry_across_rename() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("from");
        let to = temp.path().join("to");
        fs::write(&from, b"x").unwrap();
        let before = identity(&from).unwrap();

        rename_noreplace(&from, &to).unwrap();

        assert_eq!(identity(&to).unwrap(), before);
    }

    #[test]
    fn identity_tracks_directory_across_rename() {
        let temp = tempfile::tempdir().unwrap();
        let from = temp.path().join("from-dir");
        let to = temp.path().join("to-dir");
        fs::create_dir(&from).unwrap();
        let before = identity(&from).unwrap();

        rename_noreplace(&from, &to).unwrap();

        assert_eq!(identity(&to).unwrap(), before);
    }

    #[cfg(unix)]
    #[test]
    fn embedded_nul_is_rejected_before_platform_helper() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let bad = Path::new(OsStr::from_bytes(b"bad\0path"));
        let error = rename_noreplace(bad, Path::new("unused")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(windows)]
    #[test]
    fn embedded_utf16_nul_is_rejected_before_windows_helper() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        use std::path::PathBuf;

        let bad = PathBuf::from(OsString::from_wide(&[
            b'b' as u16,
            b'a' as u16,
            b'd' as u16,
            0,
            b'x' as u16,
        ]));
        let error = rename_noreplace(&bad, Path::new("unused")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
}
