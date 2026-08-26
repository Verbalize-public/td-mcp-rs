//! `project_install_bridge` — install/override the tdmcp bridge inside a
//! packed `.toe`/`.tox` (proposal §3.5; update-existing + create-from-scratch).
//!
//! Flow (all mutation on staged copies; the caller's file is replaced only
//! after targeted re-expand verification passes):
//! copy → expand → locate `tdmcp_rs` subtree → rewrite the three bridge DAT
//! bodies (`bootstrap`, `callbacks`, `tdmcp_exec` — the exec DAT mirrors
//! callbacks) with the daemon's embedded sources → collapse → targeted
//! verify (re-expand, byte-compare rewritten DATs) → backup + atomic replace.
//! Only EXISTING DAT bodies are rewritten, so `.toc` never changes.
//!
//! Create-from-scratch (proposal §3.5 P3): when the subtree is absent it is
//! materialized by expanding the shipped `bootstrap.tox` and copying TD's own
//! grammar files into the host COMP dir + appending their `.toc` lines. No
//! `.n`/`.parm` text is hand-authored — the risk V2-0 R3 flagged is sidestepped
//! by letting TouchDesigner remain the author of its own grammar.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use tdmcp_projectio::ops;
use tdmcp_projectio::resolve::OfficialTools;
use tdmcp_projectio::runner::ProcessRunner;

/// The bridge COMP exactly as TouchDesigner packed it. Expanding this yields
/// the authoritative `tdmcp_rs` grammar for create-from-scratch installs.
/// Same artifact the daemon ships — see `scripts/pack_bootstrap_tox.md`.
const BOOTSTRAP_TOX: &[u8] = include_bytes!("../../tdmcp-daemon/embedded/bootstrap.tox");

/// Embedded source per DAT (the exec DAT mirrors callbacks).
const SOURCES: [(&str, &str); 3] = [
    ("bootstrap.text", "bootstrap.py"),
    ("callbacks.text", "tox_callbacks.py"),
    ("tdmcp_exec.text", "tox_callbacks.py"),
];

fn embedded_source(name: &str) -> Vec<u8> {
    // tdmcp-mcp embeds the same bridge/ tree the daemon ships.
    match name {
        "bootstrap.py" => include_bytes!("../../../bridge/bootstrap.py").to_vec(),
        _ => include_bytes!("../../../bridge/tox_callbacks.py").to_vec(),
    }
}

/// Args for `project_install_bridge`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectInstallBridgeParams {
    /// Packed project to modify.
    pub target_path: String,
    /// `ensure` skips when payloads already match; `force` always rewrites.
    #[serde(default)]
    pub strategy: Strategy,
    /// Write `<name>.<ts>.bak` next to the target before replacing.
    #[serde(default = "default_backup")]
    pub backup: bool,
}

fn default_backup() -> bool {
    true
}

/// Install strategy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Strategy {
    /// Rewrite only when payloads differ from embedded sources.
    Ensure,
    /// Always rewrite.
    #[default]
    Force,
}

/// Locate an existing `tdmcp_rs` COMP dir anywhere under `dir`. Shared with
/// `project_lint` — the bridge sits under `project1` in a `.toe` but at the
/// root COMP of a `.tox`, so neither may hardcode a path. First match wins;
/// a project carrying two bridges is already broken and neither caller can
/// repair it.
pub(crate) fn find_subtree(dir: &Path) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let cand = d.join("tdmcp_rs");
        if cand.is_dir() && cand.join("bootstrap.text").exists() {
            return Some(cand);
        }
        if let Ok(entries) = std::fs::read_dir(&d) {
            for e in entries.flatten() {
                if e.path().is_dir() {
                    stack.push(e.path());
                }
            }
        }
    }
    None
}

/// Directory that should host a fresh `tdmcp_rs` COMP: `project1` for a
/// `.toe`, the single root COMP for a `.tox`. `None` when the expand root has
/// no unambiguous COMP to install into.
fn pick_host(expanded: &Path) -> Option<PathBuf> {
    let project1 = expanded.join("project1");
    if project1.is_dir() {
        return Some(project1);
    }
    let mut comps = std::fs::read_dir(expanded)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .map(|n| {
                        expanded
                            .join(format!("{}.n", n.to_string_lossy()))
                            .is_file()
                    })
                    .unwrap_or(false)
        });
    let only = comps.next()?;
    comps.next().is_none().then_some(only)
}

/// Materialize a `tdmcp_rs` subtree in `expanded` from the shipped
/// `bootstrap.tox`: expand it in staging, copy TD's own grammar files under the
/// host COMP, append the prefixed `.toc` lines (strict LF). Returns the new
/// subtree dir.
fn create_subtree(
    expanded: &Path,
    toc_path: &Path,
    stage: &Path,
    tools: &OfficialTools,
    runner: &ProcessRunner,
) -> Result<PathBuf, (String, &'static str)> {
    let host = pick_host(expanded).ok_or((
        "no tdmcp_rs COMP subtree, and no unambiguous host COMP to create one in".to_string(),
        "project.bridge_subtree_missing",
    ))?;
    let prefix = host
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let tox_stage = stage.join("tox");
    std::fs::create_dir_all(&tox_stage)
        .map_err(|e| (format!("tox stage mkdir: {e}"), "project.io_failed"))?;
    let tox = tox_stage.join("bootstrap.tox");
    std::fs::write(&tox, BOOTSTRAP_TOX)
        .map_err(|e| (format!("write staged tox: {e}"), "project.io_failed"))?;
    let src = ops::expand(&tox, tools, runner).map_err(|e| (format!("{e}"), code_for(&e)))?;

    let entries =
        tdmcp_projectio::toc::parse(&src.toc).map_err(|e| (format!("{e}"), code_for(&e)))?;
    let mut added = Vec::new();
    for entry in entries {
        // The tox's own `.build` is not ours to copy; the target has one.
        if entry == ".build" {
            continue;
        }
        let dst = host.join(&entry);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                (
                    format!("mkdir {}: {e}", parent.display()),
                    "project.io_failed",
                )
            })?;
        }
        std::fs::copy(src.dir.join(&entry), &dst)
            .map_err(|e| (format!("copy {entry}: {e}"), "project.io_failed"))?;
        added.push(format!("{prefix}/{entry}"));
    }

    // A half-present bridge (`tdmcp_rs.n` on disk but no subtree) already has
    // some of these lines; a duplicated toc entry is a lint error and is not
    // something toecollapse is documented to survive.
    let listed: std::collections::HashSet<String> = tdmcp_projectio::toc::parse(toc_path)
        .map_err(|e| (format!("{e}"), code_for(&e)))?
        .into_iter()
        .collect();
    let mut toc_bytes =
        std::fs::read(toc_path).map_err(|e| (format!("read toc: {e}"), "project.io_failed"))?;
    if !toc_bytes.ends_with(b"\n") {
        toc_bytes.push(b'\n');
    }
    for line in added.iter().filter(|l| !listed.contains(*l)) {
        toc_bytes.extend_from_slice(line.as_bytes());
        toc_bytes.push(b'\n');
    }
    std::fs::write(toc_path, toc_bytes)
        .map_err(|e| (format!("write toc: {e}"), "project.io_failed"))?;

    Ok(host.join("tdmcp_rs"))
}

/// Execute an install. Returns the wire payload.
pub fn run(
    params: &ProjectInstallBridgeParams,
    tools: &OfficialTools,
) -> Result<Value, (String, &'static str)> {
    use tdmcp_projectio::sidecar;
    let runner = ProcessRunner;
    let target = Path::new(&params.target_path);
    if !target.is_file() {
        return Err((
            format!("target not found: {}", target.display()),
            "project.source_not_found",
        ));
    }

    // Stage 1: private working copies.
    let stage_root = target.parent().unwrap_or(target).join(format!(
        ".tdmcp-install-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    let cleanup = |base: &Path| {
        let _ = std::fs::remove_dir_all(base);
        let _ = std::fs::remove_file(base.with_extension("tmp.toe"));
    };
    std::fs::create_dir_all(&stage_root)
        .map_err(|e| (format!("stage mkdir: {e}"), "project.io_failed"))?;

    // Keep the target's extension: toeexpand rejects a `.tox` named `.toe`.
    let work_packed = stage_root.join(format!(
        "work.{}",
        target
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("toe")
    ));
    std::fs::copy(target, &work_packed).map_err(|e| {
        cleanup(&stage_root);
        (format!("stage copy failed: {e}"), "project.io_failed")
    })?;

    // Stage 2: expand working copy beside itself.
    let outcome = ops::expand(&work_packed, tools, &runner).map_err(|e| {
        cleanup(&stage_root);
        (format!("{e}"), code_for(&e))
    })?;
    let expanded = outcome.dir;

    // Stage 3: locate subtree (creating it from the shipped tox when absent)
    // + rewrite three DAT bodies.
    let mut created = false;
    let subtree = match find_subtree(&expanded) {
        Some(found) => found,
        None => {
            created = true;
            create_subtree(&expanded, &outcome.toc, &stage_root, tools, &runner)
                .inspect_err(|_| cleanup(&stage_root))?
        }
    };
    let mut changed = Vec::new();
    for (dat, src_name) in SOURCES {
        let p = subtree.join(dat);
        let original = std::fs::read(&p).map_err(|e| {
            cleanup(&stage_root);
            (format!("read {dat}: {e}"), "project.io_failed")
        })?;
        let current_payload = sidecar::normalize_lf(sidecar::parse(&original));
        let wanted = sidecar::normalize_lf(embedded_source(src_name));
        // A freshly created subtree always goes through the write+verify path,
        // even under `ensure` - nothing existed to be "already current".
        if !created && params.strategy == Strategy::Ensure && current_payload == wanted {
            continue;
        }
        std::fs::write(&p, sidecar::encode(&wanted)).map_err(|e| {
            cleanup(&stage_root);
            (format!("write {dat}: {e}"), "project.io_failed")
        })?;
        changed.push(dat);
    }
    if changed.is_empty() && params.strategy == Strategy::Ensure {
        cleanup(&stage_root);
        return Ok(json!({
            "ok": true, "updated": false,
            "message": "bridge payloads already match embedded sources",
        }));
    }

    // Stage 4a: drop the pre-rewrite packed copy so toecollapse can write it.
    std::fs::remove_file(&work_packed).map_err(|e| {
        cleanup(&stage_root);
        (format!("unstage removal: {e}"), "project.io_failed")
    })?;

    // Stage 4b: collapse working dir back over work.toe (same-name staging).
    let collapsed = ops::collapse(&expanded, &work_packed, tools, &runner).map_err(|e| {
        cleanup(&stage_root);
        (format!("{e}"), code_for(&e))
    })?;

    // Stage 5: targeted verification — re-expand and byte-compare rewritten
    // DATs against intended payloads.
    let verify_dir = stage_root.join("verify");
    std::fs::create_dir_all(&verify_dir).map_err(|e| {
        cleanup(&stage_root);
        (format!("verify mkdir: {e}"), "project.io_failed")
    })?;
    let vpacked = verify_dir.join(
        work_packed
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("v.toe")),
    );
    std::fs::copy(&collapsed.out, &vpacked).map_err(|e| {
        cleanup(&stage_root);
        (format!("verify copy: {e}"), "project.io_failed")
    })?;
    let _ = ops::expand(&vpacked, tools, &runner).map_err(|e| {
        cleanup(&stage_root);
        (format!("{e}"), code_for(&e))
    })?;
    let vdir = PathBuf::from(format!("{}.dir", vpacked.display()));
    let vsubtree = find_subtree(&vdir).ok_or_else(|| {
        cleanup(&stage_root);
        (
            "verification lost subtree".into(),
            "project.bridge_subtree_missing",
        )
    })?;
    for (dat, src_name) in SOURCES.iter().filter(|(dat, _)| changed.contains(dat)) {
        let got = std::fs::read(vsubtree.join(dat)).map_err(|e| {
            cleanup(&stage_root);
            (format!("verify read {dat}: {e}"), "project.io_failed")
        })?;
        let payload = sidecar::parse(&got);
        let want = embedded_source(src_name);
        if sidecar::normalize_lf(payload.clone()) != sidecar::normalize_lf(want) {
            cleanup(&stage_root);
            return Err((
                format!("round-trip mismatch in {dat}"),
                "project.roundtrip_mismatch",
            ));
        }
    }

    // Stage 6: backup + publish.
    if params.backup {
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let bak = target.with_extension(format!(
            "{}.{stamp}.bak",
            target
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("toe")
        ));
        std::fs::copy(target, &bak).map_err(|e| {
            cleanup(&stage_root);
            (format!("backup failed: {e}"), "project.backup_failed")
        })?;
    }
    std::fs::copy(&collapsed.out, target).map_err(|e| {
        cleanup(&stage_root);
        (format!("publish copy: {e}"), "project.io_failed")
    })?;
    cleanup(&stage_root);

    Ok(json!({
        "ok": true,
        "updated": true,
        "created": created,
        "rewritten": changed,
        "bytes": collapsed.bytes,
    }))
}

fn code_for(e: &tdmcp_projectio::ProjectIoError) -> &'static str {
    use tdmcp_projectio::ProjectIoError::*;
    match e {
        Fs { .. } => "project.io_failed",
        SourceNotFound(_) => "project.source_not_found",
        NotPackedFormat(_) => "project.not_packed_format",
        ToolMissing { .. } | ToolPairPartial => "project.tool_missing",
        ExpandOutputMissing { .. } | CollapseOutputMissing { .. } => "project.collapse_failed",
        DestExists(_) => "project.dest_exists",
        SrcNotExpandDir { .. } => "project.src_not_expand_dir",
        TocEscape { .. } => "project.toc_escape",
        TocInvalid { .. } => "project.toc_invalid",
        RoundtripMismatch { .. } => "project.roundtrip_mismatch",
        BackupFailed { .. } => "project.backup_failed",
        BuildSkew { .. } => "project.build_skew",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "unit tests")]
mod tests {
    use super::*;
    use std::fs;

    fn comp(root: &Path, name: &str) {
        fs::create_dir_all(root.join(name)).unwrap();
        fs::write(root.join(format!("{name}.n")), b"COMP:base\nend\n").unwrap();
    }

    #[test]
    fn host_is_project1_for_a_toe_and_the_lone_comp_for_a_tox() {
        let tmp = tempfile::tempdir().unwrap();
        // .toe shape: several root COMPs, `project1` wins.
        let toe = tmp.path().join("p.toe.dir");
        for name in ["project1", "perform", "local"] {
            comp(&toe, name);
        }
        assert_eq!(pick_host(&toe), Some(toe.join("project1")));

        // .tox shape: exactly one root COMP.
        let tox = tmp.path().join("c.tox.dir");
        comp(&tox, "widget");
        assert_eq!(pick_host(&tox), Some(tox.join("widget")));

        // Ambiguous (no project1, several COMPs) and empty roots have no host.
        comp(&tox, "gadget");
        assert_eq!(pick_host(&tox), None);
        let bare = tmp.path().join("bare.dir");
        fs::create_dir_all(&bare).unwrap();
        assert_eq!(pick_host(&bare), None);
    }
}
