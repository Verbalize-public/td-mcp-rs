//! `project_install_bridge` — install/override the tdmcp bridge inside a
//! packed `.toe`/`.tox` (proposal §3.5, V2-F scope: update-existing).
//!
//! Flow (all mutation on staged copies; the caller's file is replaced only
//! after targeted re-expand verification passes):
//! copy → expand → locate `tdmcp_rs` subtree → rewrite the three bridge DAT
//! bodies (`bootstrap`, `callbacks`, `tdmcp_exec` — the exec DAT mirrors
//! callbacks) with the daemon's embedded sources → collapse → targeted
//! verify (re-expand, byte-compare rewritten DATs) → backup + atomic replace.
//! Only EXISTING DAT bodies are rewritten, so `.toc` never changes.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use tdmcp_projectio::ops;
use tdmcp_projectio::resolve::OfficialTools;
use tdmcp_projectio::runner::ProcessRunner;

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

fn find_subtree(dir: &Path) -> Option<PathBuf> {
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

    let work_packed = stage_root.join("work.toe");
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

    // Stage 3: locate subtree + rewrite three DAT bodies.
    let subtree = find_subtree(&expanded).ok_or_else(|| {
        cleanup(&stage_root);
        (
            "no tdmcp_rs COMP subtree found — drop bootstrap.tox once via live operate".into(),
            "project.bridge_subtree_missing",
        )
    })?;
    let mut changed = Vec::new();
    for (dat, src_name) in SOURCES {
        let p = subtree.join(dat);
        let original = std::fs::read(&p).map_err(|e| {
            cleanup(&stage_root);
            (format!("read {dat}: {e}"), "project.io_failed")
        })?;
        let current_payload = sidecar::normalize_lf(sidecar::parse(&original));
        let wanted = sidecar::normalize_lf(embedded_source(src_name));
        if params.strategy == Strategy::Ensure && current_payload == wanted {
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

    // Stage 4: collapse working dir back over work.toe (same-name staging).
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
    let vpacked = verify_dir.join("v.toe");
    std::fs::copy(&collapsed.out, &vpacked).map_err(|e| {
        cleanup(&stage_root);
        (format!("verify copy: {e}"), "project.io_failed")
    })?;
    let _ = ops::expand(&vpacked, tools, &runner).map_err(|e| {
        cleanup(&stage_root);
        (format!("{e}"), code_for(&e))
    })?;
    let vsubtree = find_subtree(&vpacked).ok_or_else(|| {
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
