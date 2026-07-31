//! Embed runtime assets and materialize them into the OS data directory.
//!
//! Release binaries must not depend on the git checkout. Assets are compiled
//! into the binary and extracted on first run or version bump.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use include_dir::{include_dir, Dir, DirEntry};

/// Embedded bridge package (repo `bridge/` at compile time).
static BRIDGE: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../bridge");

/// Embedded diagnostics catalog.
const CATALOG_YAML: &str = include_str!("../../../diagnostics/catalog.yaml");

/// Shipped bootstrap tox (thin dialer — handshake → FS load of `bridge/`).
const BOOTSTRAP_TOX: &[u8] = include_bytes!("../embedded/bootstrap.tox");

const STAMP_NAME: &str = "install.version";

/// Result of an install / ensure-assets pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Assets already match this binary version.
    AlreadyCurrent,
    /// Assets were (re)extracted.
    Extracted,
}

/// Ensure `{data_dir}` contains bridge/, diagnostics/catalog.yaml, bootstrap.tox,
/// and an `install.version` stamp matching this binary.
///
/// When `force` is true, always re-extract even if the stamp and marker files
/// already match this binary version (same-version bridge/catalog refresh).
pub fn ensure_installed(data_dir: &Path, force: bool) -> Result<InstallOutcome> {
    fs::create_dir_all(data_dir)
        .with_context(|| format!("create data dir {}", data_dir.display()))?;

    let stamp_path = data_dir.join(STAMP_NAME);
    let version = env!("CARGO_PKG_VERSION");
    if !force && assets_current(data_dir, &stamp_path, version) {
        return Ok(InstallOutcome::AlreadyCurrent);
    }

    extract_all(data_dir)?;
    fs::write(&stamp_path, version)
        .with_context(|| format!("write install stamp {}", stamp_path.display()))?;
    Ok(InstallOutcome::Extracted)
}

fn assets_current(data_dir: &Path, stamp_path: &Path, version: &str) -> bool {
    let Ok(stamp) = fs::read_to_string(stamp_path) else {
        return false;
    };
    if stamp.trim() != version {
        return false;
    }
    data_dir.join("bridge").join("manifest.json").is_file()
        && data_dir.join("diagnostics").join("catalog.yaml").is_file()
        && data_dir.join("bootstrap.tox").is_file()
}

fn extract_all(data_dir: &Path) -> Result<()> {
    let bridge_dir = data_dir.join("bridge");
    if bridge_dir.exists() {
        fs::remove_dir_all(&bridge_dir)
            .with_context(|| format!("remove old bridge {}", bridge_dir.display()))?;
    }
    fs::create_dir_all(&bridge_dir)
        .with_context(|| format!("create bridge dir {}", bridge_dir.display()))?;
    extract_dir(&BRIDGE, &bridge_dir)?;

    let diag_dir = data_dir.join("diagnostics");
    fs::create_dir_all(&diag_dir)
        .with_context(|| format!("create diagnostics dir {}", diag_dir.display()))?;
    let catalog_path = diag_dir.join("catalog.yaml");
    fs::write(&catalog_path, CATALOG_YAML)
        .with_context(|| format!("write catalog {}", catalog_path.display()))?;

    let tox_path = data_dir.join("bootstrap.tox");
    fs::write(&tox_path, BOOTSTRAP_TOX)
        .with_context(|| format!("write bootstrap tox {}", tox_path.display()))?;

    // Also ship bootstrap.py next to the tox for the interim dialer path.
    if let Some(entry) = BRIDGE.get_file("bootstrap.py") {
        let py_path = data_dir.join("bootstrap.py");
        fs::write(&py_path, entry.contents())
            .with_context(|| format!("write bootstrap.py {}", py_path.display()))?;
    }

    Ok(())
}

fn extract_dir(dir: &Dir<'_>, dest_root: &Path) -> Result<()> {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(subdir) => {
                if should_skip_path(subdir.path()) {
                    continue;
                }
                let child = dest_root.join(subdir.path());
                fs::create_dir_all(&child).with_context(|| format!("mkdir {}", child.display()))?;
                extract_dir(subdir, dest_root)?;
            }
            DirEntry::File(file) => {
                if should_skip_path(file.path()) {
                    continue;
                }
                let out = dest_root.join(file.path());
                if let Some(parent) = out.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("mkdir {}", parent.display()))?;
                }
                fs::write(&out, file.contents())
                    .with_context(|| format!("write {}", out.display()))?;
            }
        }
    }
    Ok(())
}

fn should_skip_path(rel: &Path) -> bool {
    rel.components().any(|c| {
        let s = c.as_os_str();
        s == "__pycache__" || s == "tests" || s.to_string_lossy().ends_with(".pyc")
    })
}

/// Resolve the default data directory (same as config).
pub fn default_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tdmcp-rs")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn extract_to_empty_data_dir() {
        let dir = tempdir().expect("tempdir");
        let data = dir.path();
        let outcome = ensure_installed(data, false).expect("install");
        assert_eq!(outcome, InstallOutcome::Extracted);
        assert!(data.join("bridge/manifest.json").is_file());
        assert!(data.join("diagnostics/catalog.yaml").is_file());
        assert!(data.join("bootstrap.tox").is_file());
        assert_eq!(
            fs::read_to_string(data.join("install.version"))
                .expect("stamp")
                .trim(),
            env!("CARGO_PKG_VERSION")
        );
        // Second call is a no-op.
        let again = ensure_installed(data, false).expect("reinstall");
        assert_eq!(again, InstallOutcome::AlreadyCurrent);
    }

    #[test]
    fn version_bump_reextracts() {
        let dir = tempdir().expect("tempdir");
        let data = dir.path();
        ensure_installed(data, false).expect("install");
        fs::write(data.join("install.version"), "0.0.0").expect("stamp");
        let outcome = ensure_installed(data, false).expect("reinstall");
        assert_eq!(outcome, InstallOutcome::Extracted);
        assert_eq!(
            fs::read_to_string(data.join("install.version"))
                .expect("stamp")
                .trim(),
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn force_reextracts_when_already_current() {
        let dir = tempdir().expect("tempdir");
        let data = dir.path();
        ensure_installed(data, false).expect("install");
        assert_eq!(
            ensure_installed(data, false).expect("noop"),
            InstallOutcome::AlreadyCurrent
        );
        let forced = ensure_installed(data, true).expect("force");
        assert_eq!(forced, InstallOutcome::Extracted);
        assert!(data.join("bridge/manifest.json").is_file());
        assert_eq!(
            fs::read_to_string(data.join("install.version"))
                .expect("stamp")
                .trim(),
            env!("CARGO_PKG_VERSION")
        );
    }
}
