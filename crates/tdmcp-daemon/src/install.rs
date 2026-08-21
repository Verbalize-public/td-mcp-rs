//! Embed runtime assets and materialize them into the OS data directory.
//!
//! Release binaries must not depend on the git checkout. Assets are compiled
//! into the binary and extracted on first run or version bump.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use include_dir::{include_dir, Dir, DirEntry};
use tdmcp_mcp::{RenderMode, TemplateEngine};

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
/// skills/, and an `install.version` stamp matching this binary.
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
        && data_dir
            .join("skills")
            .join("touchdesigner")
            .join("SKILL.md")
            .is_file()
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

    let skills_dir = data_dir.join("skills");
    if skills_dir.exists() {
        fs::remove_dir_all(&skills_dir)
            .with_context(|| format!("remove old skills {}", skills_dir.display()))?;
    }
    fs::create_dir_all(&skills_dir)
        .with_context(|| format!("create skills dir {}", skills_dir.display()))?;
    render_skills_to(&skills_dir)?;

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
        .join(tdmcp_config::APP_DIR_NAME)
}

/// Path to the extracted skills tree (`{data_dir}/skills`), ensuring assets exist.
pub fn skills_dir(data_dir: &Path) -> Result<PathBuf> {
    ensure_installed(data_dir, false)?;
    Ok(data_dir.join("skills"))
}

/// Copy the current binary into `{data_dir}/bin/` so the installed daemon has a
/// stable, independent path that is not the original build artifact.
///
/// Returns the absolute destination path. The swap tolerates a running daemon
/// or MCP proxy on Windows: an executing exe cannot be overwritten, but it can
/// be renamed aside, so the old binary is moved to a unique backup name first
/// and the new one takes its place. Stale backups that are no longer locked are
/// swept after the swap.
pub fn copy_daemon_binary(data_dir: &Path) -> Result<PathBuf> {
    let src = std::env::current_exe().context("resolve current_exe for install copy")?;
    let bin_dir = data_dir.join("bin");
    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("create bin dir {}", bin_dir.display()))?;

    let name = src.file_name().context("current_exe has no file name")?;
    let dest = bin_dir.join(name);

    // Skip when source and destination are the same file (already installed).
    match (fs::canonicalize(&src), fs::canonicalize(&dest)) {
        (Ok(canon_src), Ok(canon_dest)) if canon_src == canon_dest => {
            tracing::info!(
                src = %src.display(),
                dest = %dest.display(),
                "install: binary already at install location — skipping copy"
            );
            return Ok(dest);
        }
        _ => {}
    }

    sweep_old_backups(&bin_dir, name);
    replace_binary(&src, &dest)?;
    tracing::info!(
        src = %src.display(),
        dest = %dest.display(),
        "install: copied daemon binary to install location"
    );
    Ok(dest)
}

/// Replace `dest` with `src`, renaming a running `dest` aside first.
///
/// On Windows the destination exe is locked for overwrite while any process
/// executes it, but renaming it is allowed; a unique backup name avoids
/// colliding with a still-locked backup from a previous install.
fn replace_binary(src: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        let backup = unique_backup_path(dest);
        fs::rename(dest, &backup)
            .with_context(|| format!("rename {} → {}", dest.display(), backup.display()))?;
        match fs::copy(src, dest) {
            Ok(_) => {
                // Best-effort: the old image may still be locked by a running
                // process; sweep_old_backups retries on the next install.
                let _ = fs::remove_file(&backup);
                Ok(())
            }
            Err(e) => {
                // Restore the previous binary so install never leaves a gap.
                let _ = fs::rename(&backup, dest);
                Err(anyhow::anyhow!(
                    "copy daemon binary to {} failed: {e}",
                    dest.display()
                ))
            }
        }
    } else {
        fs::copy(src, dest).with_context(|| format!("copy daemon binary to {}", dest.display()))?;
        Ok(())
    }
}

/// A backup name that cannot collide with a still-locked previous backup.
fn unique_backup_path(dest: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    dest.with_file_name(format!("{name}.old-{nanos}"))
}

/// Remove `{name}.old-*` backups that are no longer locked by a running process.
fn sweep_old_backups(bin_dir: &Path, name: &std::ffi::OsStr) {
    let prefix = format!("{}.old-", name.to_string_lossy());
    let Ok(entries) = fs::read_dir(bin_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(s) = file_name.to_str() else {
            continue;
        };
        if s.starts_with(&prefix) {
            // Ignore failures: a locked backup waits for its process to exit.
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Verify the installed binary reports this build's version via `--version`.
///
/// Guards against a partial copy or a swapped-in artifact of the wrong build.
pub fn verify_installed_version(exe: &Path) -> Result<()> {
    let expected = env!("CARGO_PKG_VERSION");
    let out = std::process::Command::new(exe)
        .arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .with_context(|| format!("run {} --version", exe.display()))?;
    if !out.status.success() {
        bail!(
            "installed binary at {} failed `--version` (status {})",
            exe.display(),
            out.status
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    if !text.contains(expected) {
        bail!(
            "installed binary at {} reports {:?}; expected {}",
            exe.display(),
            text.trim(),
            expected
        );
    }
    tracing::info!(exe = %exe.display(), version = expected, "install: verified installed binary version");
    Ok(())
}
/// Render all skill cards in filesystem mode into `dest`.
///
/// Each card's cross-references become relative Markdown links (no
/// `tdmcp://docs/*` URIs). Returns `(relative_path, absolute_output_path)`
/// pairs for every file written.
pub fn render_skills_to(dest: &Path) -> Result<Vec<(String, PathBuf)>> {
    let catalog = tdmcp_mcp::Catalog::from_manifest_yaml(tdmcp_mcp::MANIFEST_YAML)
        .map_err(|e| anyhow::anyhow!("parse embedded skills MANIFEST: {e}"))?;
    let engine = TemplateEngine::new(catalog, &tdmcp_mcp::TEMPLATES)
        .map_err(|e| anyhow::anyhow!("initialize skills template engine: {e}"))?;
    let rendered = engine
        .render_all(RenderMode::FileSystem)
        .map_err(|e| anyhow::anyhow!("render skills in filesystem mode: {e}"))?;

    fs::create_dir_all(dest).with_context(|| format!("create dest {}", dest.display()))?;
    let mut written = Vec::new();
    for (rel_path, content) in rendered {
        let out = dest.join(&rel_path);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
        }
        fs::write(&out, &content).with_context(|| format!("write {}", out.display()))?;
        written.push((rel_path, out));
    }
    Ok(written)
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
        assert!(data.join("skills/touchdesigner/SKILL.md").is_file());
        assert!(data
            .join("skills/touchdesigner/reference/opsketch-notation.md")
            .is_file());
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
        assert!(data.join("skills/touchdesigner/SKILL.md").is_file());
        assert_eq!(
            fs::read_to_string(data.join("install.version"))
                .expect("stamp")
                .trim(),
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn replace_binary_swaps_running_dest() {
        let dir = tempdir().expect("tempdir");
        let src = dir.path().join("new-daemon.exe");
        let dest = dir.path().join("bin").join("tdmcp-daemon.exe");
        fs::create_dir_all(dest.parent().expect("bin parent")).expect("mkdir bin");
        fs::write(&src, b"new-binary").expect("write src");
        fs::write(&dest, b"old-binary").expect("write dest");
        replace_binary(&src, &dest).expect("replace");
        assert_eq!(fs::read(&dest).expect("read dest"), b"new-binary");
        // The old image was moved aside and dropped once unlocked.
        let leftovers = fs::read_dir(dest.parent().expect("bin"))
            .expect("read bin")
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("tdmcp-daemon.exe.old-")
            })
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn replace_binary_restores_on_copy_failure() {
        let dir = tempdir().expect("tempdir");
        let dest = dir.path().join("bin").join("tdmcp-daemon.exe");
        fs::create_dir_all(dest.parent().expect("bin parent")).expect("mkdir bin");
        fs::write(&dest, b"old-binary").expect("write dest");
        // Missing src makes the copy fail; the previous binary must come back.
        let err = replace_binary(&dir.path().join("missing.exe"), &dest).expect_err("copy fails");
        assert!(err.to_string().contains("copy daemon binary to"));
        assert_eq!(fs::read(&dest).expect("read dest"), b"old-binary");
    }

    #[test]
    fn unique_backup_paths_do_not_collide() {
        let dest = Path::new("bin").join("tdmcp-daemon.exe");
        assert_ne!(unique_backup_path(&dest), unique_backup_path(&dest));
    }

    #[test]
    fn render_skills_to_dest() {
        let dir = tempdir().expect("tempdir");
        let dest = dir.path().join("host-skills");
        let written = render_skills_to(&dest).expect("render");
        assert!(!written.is_empty());
        assert!(dest.join("touchdesigner/SKILL.md").is_file());
        // Filesystem mode must not contain any tdmcp:// resource URIs.
        let skill_body =
            fs::read_to_string(dest.join("touchdesigner/SKILL.md")).expect("read SKILL.md");
        assert!(!skill_body.contains("tdmcp://docs/"));
    }
}
