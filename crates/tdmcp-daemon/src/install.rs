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

/// Retain the project's license even when installing from a standalone binary.
const PROJECT_LICENSE: &str = include_str!("../../../LICENSE");

/// Shipped bootstrap tox (thin dialer — handshake → FS load of `bridge/`).
const BOOTSTRAP_TOX: &[u8] = include_bytes!("../embedded/bootstrap.tox");

/// Shipped template toe for `spawn_td` `createIfMissing`.
const TEMPLATE_TOE: &[u8] = include_bytes!("../embedded/template.toe");

/// FNV-1a hash of `bridge/bootstrap.py` + `bridge/tox_callbacks.py` as of the
/// last live-TD pack of `BOOTSTRAP_TOX` — see `stamp_tox_source_hash` test
/// below and `xtask stamp-tox`.
#[cfg(test)]
const TOX_SOURCE_HASH: &str = include_str!("../embedded/bootstrap.tox.source-hash");

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
/// skills/, LICENSE, and an `install.version` stamp matching this binary.
///
/// When `force` is true, always re-extract even if the stamp and marker files
/// already match this binary version (same-version bridge/catalog refresh).
pub fn ensure_installed(data_dir: &Path, force: bool) -> Result<InstallOutcome> {
    fs::create_dir_all(data_dir)
        .with_context(|| format!("create data dir {}", data_dir.display()))?;

    // An OS lock survives neither a crash nor process exit. Keep the lock file
    // itself in place so concurrent installers always lock the same inode.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join("install.lock"))
        .context("open asset install lock")?;
    lock.lock().context("lock asset installation")?;

    let stamp_path = data_dir.join(STAMP_NAME);
    let version = env!("CARGO_PKG_VERSION");
    if !force && assets_current(data_dir, &stamp_path, version) {
        return Ok(InstallOutcome::AlreadyCurrent);
    }

    let stage = tempfile::Builder::new()
        .prefix(".install-stage-")
        .tempdir_in(data_dir)
        .context("stage runtime assets")?;
    extract_all(stage.path())?;
    fs::write(stage.path().join(STAMP_NAME), version).context("stage install stamp")?;
    let mut names = vec![
        "bridge",
        "skills",
        "diagnostics",
        "bootstrap.tox",
        "bootstrap.py",
        "LICENSE",
    ];
    // This file is a user-customizable project, not a managed runtime asset.
    if !data_dir.join("template.toe").exists() {
        names.push("template.toe");
    }
    names.push(STAMP_NAME);
    publish_assets(stage.path(), data_dir, &names)?;
    Ok(InstallOutcome::Extracted)
}

/// Prepare everything before touching the installation; restore old entries
/// on a normal filesystem error. This is not a crash-atomic multi-file commit.
fn publish_assets(stage: &Path, data_dir: &Path, names: &[&str]) -> Result<()> {
    let backup = tempfile::Builder::new()
        .prefix(".install-backup-")
        .tempdir_in(data_dir)
        .context("prepare asset rollback")?;
    let mut moved = Vec::new();
    let result: Result<()> = (|| {
        for name in names {
            let dest = data_dir.join(name);
            let old = dest.symlink_metadata().is_ok();
            if old {
                fs::rename(&dest, backup.path().join(name))
                    .with_context(|| format!("back up installed asset {name}"))?;
            }
            moved.push((*name, old));
            fs::rename(stage.join(name), &dest)
                .with_context(|| format!("publish installed asset {name}"))?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        for (name, old) in moved.into_iter().rev() {
            let dest = data_dir.join(name);
            if dest.symlink_metadata().is_ok() {
                // Move the new asset back into staging, never recursively
                // delete a possibly unfamiliar destination during rollback.
                if let Err(e) = fs::rename(&dest, stage.join(name)) {
                    rollback_errors.push(format!("remove replacement {name}: {e}"));
                    continue;
                }
            }
            if old {
                if let Err(e) = fs::rename(backup.path().join(name), &dest) {
                    rollback_errors.push(format!("restore {name}: {e}"));
                }
            }
        }
        if !rollback_errors.is_empty() {
            let recovery = backup.keep();
            return Err(error.context(format!(
                "rollback incomplete ({}); previous assets retained at {}",
                rollback_errors.join("; "),
                recovery.display()
            )));
        }
        return Err(error.context("previous runtime assets restored"));
    }
    Ok(())
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
        && data_dir.join("template.toe").is_file()
        && data_dir.join("LICENSE").is_file()
        && data_dir
            .join("skills")
            .join("touchdesigner")
            .join("SKILL.md")
            .is_file()
}

fn extract_all(data_dir: &Path) -> Result<()> {
    fs::write(data_dir.join("LICENSE"), PROJECT_LICENSE).context("write project license")?;
    let bridge_dir = data_dir.join("bridge");
    fs::create_dir_all(&bridge_dir)
        .with_context(|| format!("create bridge dir {}", bridge_dir.display()))?;
    extract_dir(&BRIDGE, &bridge_dir)?;

    let skills_dir = data_dir.join("skills");
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

    // Users can customize this starter from the dashboard. Never erase their
    // project on an upgrade or forced refresh of managed assets.
    let template_path = data_dir.join("template.toe");
    if !template_path.exists() {
        fs::write(&template_path, TEMPLATE_TOE)
            .with_context(|| format!("write template toe {}", template_path.display()))?;
    }

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
    let parent = dest.parent().context("installed binary has no parent")?;
    let stage = tempfile::NamedTempFile::new_in(parent)
        .context("stage daemon binary")?
        .into_temp_path();
    fs::copy(src, &stage).with_context(|| format!("copy daemon binary to {}", dest.display()))?;
    if dest.exists() {
        let backup = unique_backup_path(dest);
        fs::rename(dest, &backup)
            .with_context(|| format!("rename {} → {}", dest.display(), backup.display()))?;
        match fs::rename(&stage, dest) {
            Ok(()) => {
                // Best-effort: the old image may still be locked by a running
                // process; sweep_old_backups retries on the next install.
                let _ = fs::remove_file(&backup);
                Ok(())
            }
            Err(e) => {
                // Restore the previous binary so install never leaves a gap.
                if let Err(restore) = fs::rename(&backup, dest) {
                    bail!("publish daemon binary failed: {e}; restore failed: {restore}; previous binary retained at {}", backup.display());
                }
                Err(anyhow::anyhow!(
                    "publish daemon binary to {} failed: {e}",
                    dest.display()
                ))
            }
        }
    } else {
        fs::rename(&stage, dest)
            .with_context(|| format!("publish daemon binary to {}", dest.display()))?;
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
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests"
)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn standalone_install_includes_license_and_repairs_older_installation() {
        let data = tempdir().unwrap();
        assert_eq!(
            ensure_installed(data.path(), false).unwrap(),
            InstallOutcome::Extracted
        );
        let license = data.path().join("LICENSE");
        assert_eq!(fs::read_to_string(&license).unwrap(), PROJECT_LICENSE);
        fs::remove_file(&license).unwrap();
        assert_eq!(
            ensure_installed(data.path(), false).unwrap(),
            InstallOutcome::Extracted
        );
        assert_eq!(fs::read_to_string(&license).unwrap(), PROJECT_LICENSE);
        assert_eq!(
            ensure_installed(data.path(), false).unwrap(),
            InstallOutcome::AlreadyCurrent
        );
    }

    /// `bootstrap.tox` is TouchDesigner's opaque binary component format —
    /// nothing outside TD can parse or diff its contents against source. It
    /// is packed from `bridge/bootstrap.py` + `bridge/tox_callbacks.py` by a
    /// **manual, live-TD-only** step (`scripts/pack_bootstrap_tox.md`), so
    /// editing either `.py` file and forgetting to repack silently ships a
    /// stale tox with no error anywhere — this is exactly that guard. The
    /// FNV-1a hash is a drift check, not a content check: it can't tell you
    /// the tox is *correct*, only that it was packed after these two files
    /// last looked like this. Failing here means: re-run the packing script
    /// in a live TD session, save over `embedded/bootstrap.tox`, then run
    /// `cargo run -p xtask -- stamp-tox` to record the new hash.
    #[test]
    fn bootstrap_tox_matches_packed_source_hash() {
        let bootstrap = BRIDGE
            .get_file("bootstrap.py")
            .expect("bootstrap.py embedded in BRIDGE")
            .contents();
        let callbacks = BRIDGE
            .get_file("tox_callbacks.py")
            .expect("tox_callbacks.py embedded in BRIDGE")
            .contents();
        let hash = fnv1a(&[&normalize_eol(bootstrap), &normalize_eol(callbacks)]);
        let stored = TOX_SOURCE_HASH.trim();
        assert_eq!(
            format!("{hash:016x}"),
            stored,
            "bridge/bootstrap.py or bridge/tox_callbacks.py changed since \
             crates/tdmcp-daemon/embedded/bootstrap.tox was last packed. Re-run the \
             live-TD packing script in scripts/pack_bootstrap_tox.md, save over \
             embedded/bootstrap.tox, then run `cargo run -p xtask -- stamp-tox`."
        );
    }

    /// EOL-normalize before hashing so a stamp taken on a CRLF Windows
    /// checkout matches the LF bytes unix CI embeds — mirrors xtask's
    /// `normalize_eol` exactly.
    fn normalize_eol(bytes: &[u8]) -> Vec<u8> {
        let text = String::from_utf8_lossy(bytes);
        if text.contains('\r') {
            text.replace("\r\n", "\n").into_bytes()
        } else {
            bytes.to_vec()
        }
    }

    /// Same algorithm as `xtask`'s `fnv1a` — deliberately duplicated rather
    /// than shared (10 lines, used in exactly these two places; a shared
    /// crate for this would be the over-engineering, not the duplication).
    fn fnv1a(chunks: &[&[u8]]) -> u64 {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut hash = OFFSET;
        for chunk in chunks {
            for &byte in *chunk {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(PRIME);
            }
        }
        hash
    }

    #[test]
    fn extract_to_empty_data_dir() {
        let dir = tempdir().expect("tempdir");
        let data = dir.path();
        let outcome = ensure_installed(data, false).expect("install");
        assert_eq!(outcome, InstallOutcome::Extracted);
        assert!(data.join("bridge/manifest.json").is_file());
        assert!(data.join("diagnostics/catalog.yaml").is_file());
        assert!(data.join("bootstrap.tox").is_file());
        assert!(data.join("template.toe").is_file());
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
    fn template_toe_present_after_install() {
        let dir = tempdir().expect("tempdir");
        let data = dir.path();
        ensure_installed(data, false).expect("install");
        assert!(data.join("template.toe").is_file());
        assert!(fs::read(data.join("template.toe")).expect("read").len() > 512);
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
    fn force_preserves_custom_template() {
        let tmp = tempfile::tempdir().expect("tempdir");
        ensure_installed(tmp.path(), false).expect("install");
        let template = tmp.path().join("template.toe");
        fs::write(&template, b"user's customized project").expect("customize");
        ensure_installed(tmp.path(), true).expect("refresh");
        assert_eq!(
            fs::read(template).expect("read"),
            b"user's customized project"
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
    fn asset_publish_failure_restores_previous_entries_and_stamp() {
        let dir = tempdir().expect("tempdir");
        let data = dir.path().join("installed");
        let stage = dir.path().join("staged");
        fs::create_dir_all(data.join("bridge")).expect("old bridge");
        fs::create_dir_all(stage.join("bridge")).expect("new bridge");
        fs::write(data.join("bridge/source.py"), "old").expect("old source");
        fs::write(stage.join("bridge/source.py"), "new").expect("new source");
        fs::write(data.join("bootstrap.tox"), "old tox").expect("old tox");
        fs::write(data.join(STAMP_NAME), "old version").expect("old stamp");
        // Missing staged tox fails after the bridge has already been swapped.
        let error = publish_assets(&stage, &data, &["bridge", "bootstrap.tox", STAMP_NAME])
            .expect_err("missing staged asset");
        assert!(error
            .to_string()
            .contains("previous runtime assets restored"));
        assert_eq!(
            fs::read_to_string(data.join("bridge/source.py")).expect("source"),
            "old"
        );
        assert_eq!(
            fs::read_to_string(data.join("bootstrap.tox")).expect("tox"),
            "old tox"
        );
        assert_eq!(
            fs::read_to_string(data.join(STAMP_NAME)).expect("stamp"),
            "old version"
        );
    }

    #[test]
    fn concurrent_asset_installers_publish_once() {
        let dir = tempdir().expect("tempdir");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let data = dir.path().to_path_buf();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    ensure_installed(&data, false).expect("concurrent install")
                })
            })
            .collect();
        let extracted = workers
            .into_iter()
            .filter_map(|worker| {
                (worker.join().expect("worker") == InstallOutcome::Extracted).then_some(())
            })
            .count();
        assert_eq!(extracted, 1);
        assert!(assets_current(
            dir.path(),
            &dir.path().join(STAMP_NAME),
            env!("CARGO_PKG_VERSION")
        ));
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

    /// The Claude Code plugin (`.claude-plugin/plugin.json`) ships a
    /// pre-rendered copy of the skill pack at repo-root `claude-skills/` —
    /// plugin installs are a git checkout with no build step, so the
    /// rendered Markdown must already be sitting in the tree. That makes it
    /// a checked-in artifact of `skills/templates/**/*.jinja.md` +
    /// `skills/MANIFEST.yaml`, exactly like `bootstrap.tox` is a checked-in
    /// artifact of `bridge/*.py` (see `bootstrap_tox_matches_packed_source_hash`
    /// above) — nothing enforces they stay in sync except this test. Failing
    /// here means a skill card changed without re-running
    /// `cargo run -p tdmcp-daemon -- skills render --dest claude-skills`
    /// and committing the result.
    #[test]
    fn claude_plugin_skills_match_rendered_output() {
        let checked_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../claude-skills")
            .canonicalize()
            .expect("claude-skills/ exists at repo root");

        let dir = tempdir().expect("tempdir");
        let fresh = dir.path().join("claude-skills");
        render_skills_to(&fresh).expect("render");

        let mut checked_in_files = list_files_relative(&checked_in);
        let mut fresh_files = list_files_relative(&fresh);
        checked_in_files.sort();
        fresh_files.sort();
        assert_eq!(
            checked_in_files, fresh_files,
            "claude-skills/ file list is stale — re-render with \
             `cargo run -p tdmcp-daemon -- skills render --dest claude-skills` and commit"
        );

        for rel in &fresh_files {
            let checked_in_content = fs::read_to_string(checked_in.join(rel))
                .unwrap_or_else(|_| panic!("read checked-in claude-skills/{}", rel.display()));
            let fresh_content = fs::read_to_string(fresh.join(rel))
                .unwrap_or_else(|_| panic!("read freshly rendered {}", rel.display()));
            assert_eq!(
                checked_in_content,
                fresh_content,
                "claude-skills/{} is stale — re-render with \
                 `cargo run -p tdmcp-daemon -- skills render --dest claude-skills` and commit",
                rel.display()
            );
        }
    }

    /// Relative paths (from `root`) of every file under `root`, recursively.
    fn list_files_relative(root: &Path) -> Vec<PathBuf> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(dir).expect("read_dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else {
                    out.push(
                        path.strip_prefix(root)
                            .expect("path under root")
                            .to_path_buf(),
                    );
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out
    }
}
