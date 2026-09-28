//! `project_install_bridge` — install/override the tdmcp bridge inside a
//! packed `.toe`/`.tox` (proposal §3.5; update-existing + create-from-scratch).
//!
//! Flow (all mutation on staged copies; the caller's file is replaced only
//! after full re-expand verification passes):
//! copy → expand → locate the bridge subtree → rewrite the three bridge DAT
//! bodies (`bootstrap`, `callbacks`, `tdmcp_exec` — the exec DAT mirrors
//! callbacks) with the daemon's embedded sources → snapshot → collapse →
//! verify the full manifest, every untouched entry, and all bridge bodies →
//! backup + publish. Existing bridges keep their `.toc` unchanged.
//!
//! Create-from-scratch (proposal §3.5 P3): when the subtree is absent it is
//! materialized by expanding the shipped `bootstrap.tox` and copying TD's own
//! grammar files into the host COMP dir + appending their `.toc` lines. No
//! `.n`/`.parm` text is hand-authored — the risk V2-0 R3 flagged is sidestepped
//! by letting TouchDesigner remain the author of its own grammar.

use std::collections::HashSet;
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
const BRIDGE_DATS: [&str; 3] = ["bootstrap.text", "callbacks.text", "tdmcp_exec.text"];

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

/// A bridge layout that cannot be selected without risking unrelated data.
#[derive(Debug)]
pub(crate) enum BridgeLayoutError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Partial {
        paths: Vec<PathBuf>,
    },
    Ambiguous {
        paths: Vec<PathBuf>,
    },
}

impl BridgeLayoutError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Io { .. } => "project.io_failed",
            Self::Partial { .. } | Self::Ambiguous { .. } => "project.bridge_subtree_missing",
        }
    }

    pub(crate) fn paths(&self) -> &[PathBuf] {
        match self {
            Self::Io { path, .. } => std::slice::from_ref(path),
            Self::Partial { paths } | Self::Ambiguous { paths } => paths,
        }
    }
}

impl core::fmt::Display for BridgeLayoutError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "bridge discovery failed at {}: {source}", path.display())
            }
            Self::Partial { paths } => write!(
                f,
                "partial bridge candidate(s) prevent safe repair: {}",
                display_paths(paths)
            ),
            Self::Ambiguous { paths } => write!(
                f,
                "multiple complete bridge candidates are ambiguous: {}",
                display_paths(paths)
            ),
        }
    }
}

impl std::error::Error for BridgeLayoutError {}

fn display_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn has_bridge_payload_marker(path: &Path) -> std::io::Result<bool> {
    use std::io::Read;

    const MARKER: &[u8] = b"tdmcp_bridge";
    const LIMIT: u64 = 64 * 1024;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(LIMIT)
        .read_to_end(&mut bytes)?;
    Ok(bytes.windows(MARKER.len()).any(|window| window == MARKER))
}

/// Locate exactly one complete bridge COMP anywhere under `dir`.
///
/// The COMP may have been renamed: a renamed candidate needs its TD node
/// sidecar plus a positive `tdmcp_bridge` payload marker in all three bridge DAT
/// bodies. The canonical `tdmcp_rs` name is itself reserved and any incomplete
/// canonical layout is a partial collision. Marker-free generic DATs are left
/// alone. Positive partial or duplicate candidates are rejected instead of
/// choosing a first match or copying over uncertain data.
pub(crate) fn find_subtree(dir: &Path) -> Result<Option<PathBuf>, BridgeLayoutError> {
    fn scan(
        dir: &Path,
        complete: &mut Vec<PathBuf>,
        partial: &mut Vec<PathBuf>,
    ) -> Result<(), BridgeLayoutError> {
        let read = std::fs::read_dir(dir).map_err(|source| BridgeLayoutError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let mut entries =
            read.collect::<Result<Vec<_>, _>>()
                .map_err(|source| BridgeLayoutError::Io {
                    path: dir.to_path_buf(),
                    source,
                })?;
        entries.sort_by_key(std::fs::DirEntry::file_name);

        let canonical_node = dir.join("tdmcp_rs.n");
        if canonical_node.is_file() && !dir.join("tdmcp_rs").is_dir() {
            partial.push(canonical_node);
        }

        for entry in entries {
            let file_type = entry.file_type().map_err(|source| BridgeLayoutError::Io {
                path: entry.path(),
                source,
            })?;
            if !file_type.is_dir() {
                continue;
            }
            let path = entry.path();
            let dat_count = BRIDGE_DATS
                .iter()
                .filter(|dat| path.join(dat).is_file())
                .count();
            let mut marker_count = 0;
            for dat in BRIDGE_DATS {
                let dat_path = path.join(dat);
                if dat_path.is_file()
                    && has_bridge_payload_marker(&dat_path).map_err(|source| {
                        BridgeLayoutError::Io {
                            path: dat_path.clone(),
                            source,
                        }
                    })?
                {
                    marker_count += 1;
                }
            }
            let canonical_name = path
                .file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new("tdmcp_rs"));
            let has_node = path.file_name().is_some_and(|name| {
                path.parent().is_some_and(|parent| {
                    parent
                        .join(format!("{}.n", name.to_string_lossy()))
                        .is_file()
                })
            });
            if has_node
                && dat_count == BRIDGE_DATS.len()
                && (canonical_name || marker_count == BRIDGE_DATS.len())
            {
                complete.push(path.clone());
            } else if canonical_name || marker_count > 0 {
                partial.push(path.clone());
            }
            scan(&path, complete, partial)?;
        }
        Ok(())
    }

    let mut complete = Vec::new();
    let mut partial = Vec::new();
    scan(dir, &mut complete, &mut partial)?;
    complete.sort();
    partial.sort();

    if complete.len() > 1 {
        complete.extend(partial);
        complete.sort();
        return Err(BridgeLayoutError::Ambiguous { paths: complete });
    }
    if !partial.is_empty() {
        if let Some(path) = complete.pop() {
            partial.push(path);
            partial.sort();
        }
        return Err(BridgeLayoutError::Partial { paths: partial });
    }
    Ok(complete.pop())
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

/// Materialize a canonical `tdmcp_rs` subtree from one expanded donor.
///
/// The donor root name is discovered from its complete bridge layout and every
/// listed root path is deliberately remapped. The copy plan and all target
/// collisions are validated before the staging tree is mutated.
fn materialize_donor(
    expanded: &Path,
    toc_path: &Path,
    donor_dir: &Path,
    donor_toc: &Path,
) -> Result<PathBuf, (String, &'static str)> {
    let host = pick_host(expanded).ok_or((
        "no bridge subtree, and no unambiguous host COMP to create one in".to_string(),
        "project.bridge_subtree_missing",
    ))?;
    let prefix = host
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let donor_entries = tdmcp_projectio::toc::parse(donor_toc)
        .map_err(|e| (format!("invalid bridge donor toc: {e}"), code_for(&e)))?;
    tdmcp_projectio::toc::validate_entries(donor_dir, &donor_entries)
        .map_err(|e| (format!("invalid bridge donor toc: {e}"), code_for(&e)))?;
    let donor_bridge = find_subtree(donor_dir)
        .map_err(|e| (format!("invalid bridge donor layout: {e}"), e.code()))?
        .ok_or((
            "bridge donor contains no complete bridge subtree".to_string(),
            "project.bridge_subtree_missing",
        ))?;
    if donor_bridge.parent() != Some(donor_dir) {
        return Err((
            format!(
                "bridge donor root must be a direct expand-root child, found {}",
                donor_bridge.display()
            ),
            "project.bridge_subtree_missing",
        ));
    }
    let donor_name = donor_bridge
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or((
            "bridge donor root name is not valid UTF-8".to_string(),
            "project.toc_invalid",
        ))?;
    let donor_node = format!("{donor_name}.n");
    if !donor_entries.iter().any(|entry| entry == &donor_node) {
        return Err((
            format!("bridge donor toc does not list root node {donor_node}"),
            "project.bridge_subtree_missing",
        ));
    }

    let target_entries = tdmcp_projectio::toc::parse(toc_path)
        .map_err(|e| (format!("invalid target toc: {e}"), code_for(&e)))?;
    let listed: HashSet<String> = target_entries.into_iter().collect();
    let mut mapped = HashSet::new();
    let mut copy_plan = Vec::new();
    for entry in donor_entries {
        // The donor's `.build` belongs to its packed artifact; the target keeps
        // its own build marker.
        if entry == ".build" {
            continue;
        }
        let Some(suffix) = entry.strip_prefix(donor_name) else {
            return Err((
                format!("bridge donor toc has unrelated entry {entry}"),
                "project.toc_invalid",
            ));
        };
        if !suffix.starts_with('/') && !suffix.starts_with('.') {
            return Err((
                format!("bridge donor toc has unrelated entry {entry}"),
                "project.toc_invalid",
            ));
        }
        let remapped = format!("tdmcp_rs{suffix}");
        if !mapped.insert(remapped.clone()) {
            return Err((
                format!("bridge donor remaps duplicate entry {entry}"),
                "project.toc_invalid",
            ));
        }
        let source = tdmcp_projectio::toc::entry_path(donor_dir, &entry);
        if !source.is_file() {
            return Err((
                format!("bridge donor entry missing on disk: {entry}"),
                "project.toc_invalid",
            ));
        }
        let destination = tdmcp_projectio::toc::entry_path(&host, &remapped);
        let target_entry = format!("{prefix}/{remapped}");
        if destination.exists() || listed.contains(&target_entry) {
            return Err((
                format!(
                    "partial bridge collision at {}; refusing to overwrite or duplicate it",
                    destination.display()
                ),
                "project.bridge_subtree_missing",
            ));
        }
        copy_plan.push((source, destination, target_entry));
    }
    if copy_plan.is_empty() {
        return Err((
            "bridge donor has no copyable entries".to_string(),
            "project.bridge_subtree_missing",
        ));
    }

    for (source, destination, _) in &copy_plan {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                (
                    format!("mkdir {}: {e}", parent.display()),
                    "project.io_failed",
                )
            })?;
        }
        std::fs::copy(source, destination).map_err(|e| {
            (
                format!(
                    "copy {} to {}: {e}",
                    source.display(),
                    destination.display()
                ),
                "project.io_failed",
            )
        })?;
    }

    let mut toc_bytes =
        std::fs::read(toc_path).map_err(|e| (format!("read toc: {e}"), "project.io_failed"))?;
    if !toc_bytes.ends_with(b"\n") {
        toc_bytes.push(b'\n');
    }
    for (_, _, line) in &copy_plan {
        toc_bytes.extend_from_slice(line.as_bytes());
        toc_bytes.push(b'\n');
    }
    std::fs::write(toc_path, toc_bytes)
        .map_err(|e| (format!("write toc: {e}"), "project.io_failed"))?;

    let installed = host.join("tdmcp_rs");
    if !BRIDGE_DATS.iter().all(|dat| installed.join(dat).is_file()) {
        return Err((
            "bridge donor remap did not materialize all bridge DATs".to_string(),
            "project.bridge_subtree_missing",
        ));
    }
    Ok(installed)
}

/// Materialize a `tdmcp_rs` subtree in `expanded` from the shipped
/// `bootstrap.tox`: expand it in staging, discover its actual root, remap TD's
/// own grammar under the host COMP, and append strict-LF `.toc` lines.
fn create_subtree(
    expanded: &Path,
    toc_path: &Path,
    stage: &Path,
    tools: &OfficialTools,
    runner: &dyn tdmcp_projectio::runner::CommandRunner,
) -> Result<PathBuf, (String, &'static str)> {
    let tox_stage = stage.join("tox");
    std::fs::create_dir_all(&tox_stage)
        .map_err(|e| (format!("tox stage mkdir: {e}"), "project.io_failed"))?;
    let tox = tox_stage.join("bootstrap.tox");
    std::fs::write(&tox, BOOTSTRAP_TOX)
        .map_err(|e| (format!("write staged tox: {e}"), "project.io_failed"))?;
    let src = ops::expand(&tox, tools, runner).map_err(|e| (format!("{e}"), code_for(&e)))?;
    materialize_donor(expanded, toc_path, &src.dir, &src.toc)
}

fn snapshot_manifest(
    expanded: &Path,
    toc_path: &Path,
    snapshot: &Path,
) -> Result<Vec<String>, (String, &'static str)> {
    let entries = tdmcp_projectio::toc::parse(toc_path)
        .map_err(|e| (format!("snapshot toc: {e}"), code_for(&e)))?;
    tdmcp_projectio::toc::validate_entries(expanded, &entries)
        .map_err(|e| (format!("snapshot toc: {e}"), code_for(&e)))?;
    std::fs::create_dir_all(snapshot)
        .map_err(|e| (format!("snapshot mkdir: {e}"), "project.io_failed"))?;
    for entry in &entries {
        let source = tdmcp_projectio::toc::entry_path(expanded, entry);
        if !source.is_file() {
            return Err((
                format!("snapshot entry missing on disk: {entry}"),
                "project.toc_invalid",
            ));
        }
        let destination = tdmcp_projectio::toc::entry_path(snapshot, entry);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                (
                    format!("snapshot mkdir {}: {e}", parent.display()),
                    "project.io_failed",
                )
            })?;
        }
        std::fs::copy(&source, &destination).map_err(|e| {
            (
                format!(
                    "snapshot copy {} to {}: {e}",
                    source.display(),
                    destination.display()
                ),
                "project.io_failed",
            )
        })?;
    }
    Ok(entries)
}

fn files_equal(left: &Path, right: &Path) -> std::io::Result<bool> {
    use std::io::Read;

    let left_len = std::fs::metadata(left)?.len();
    if std::fs::metadata(right)?.len() != left_len {
        return Ok(false);
    }
    let mut left = std::io::BufReader::new(std::fs::File::open(left)?);
    let mut right = std::io::BufReader::new(std::fs::File::open(right)?);
    let mut left_buf = [0_u8; 64 * 1024];
    let mut right_buf = [0_u8; 64 * 1024];
    let mut remaining = left_len;
    while remaining > 0 {
        let chunk_u64 = remaining.min(64 * 1024);
        let chunk = usize::try_from(chunk_u64)
            .map_err(|_| std::io::Error::other("comparison chunk does not fit usize"))?;
        left.read_exact(&mut left_buf[..chunk])?;
        right.read_exact(&mut right_buf[..chunk])?;
        if left_buf[..chunk] != right_buf[..chunk] {
            return Ok(false);
        }
        remaining -= chunk_u64;
    }
    Ok(true)
}

fn verify_preserved_manifest(
    snapshot: &Path,
    expected_entries: &[String],
    actual_dir: &Path,
    actual_toc: &Path,
    bridge_entries: &HashSet<String>,
) -> Result<(), (String, &'static str)> {
    let actual_entries = tdmcp_projectio::toc::parse(actual_toc)
        .map_err(|e| (format!("verify toc: {e}"), code_for(&e)))?;
    if actual_entries != expected_entries {
        return Err((
            "round-trip changed the toc manifest".to_string(),
            "project.roundtrip_mismatch",
        ));
    }
    for entry in expected_entries {
        if bridge_entries.contains(entry) {
            continue;
        }
        let expected = tdmcp_projectio::toc::entry_path(snapshot, entry);
        let actual = tdmcp_projectio::toc::entry_path(actual_dir, entry);
        if !actual.is_file() {
            return Err((
                format!("round-trip lost untouched entry {entry}"),
                "project.roundtrip_mismatch",
            ));
        }
        let equal = files_equal(&expected, &actual).map_err(|e| {
            (
                format!("compare untouched entry {entry}: {e}"),
                "project.io_failed",
            )
        })?;
        if !equal {
            return Err((
                format!("round-trip changed untouched entry {entry}"),
                "project.roundtrip_mismatch",
            ));
        }
    }
    Ok(())
}

/// Execute an install. Returns the wire payload.
pub fn run(
    params: &ProjectInstallBridgeParams,
    tools: &OfficialTools,
) -> Result<Value, (String, &'static str)> {
    let runner = ProcessRunner;
    run_with_runner(params, tools, &runner)
}

fn run_with_runner(
    params: &ProjectInstallBridgeParams,
    tools: &OfficialTools,
    runner: &dyn tdmcp_projectio::runner::CommandRunner,
) -> Result<Value, (String, &'static str)> {
    use tdmcp_projectio::sidecar;
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
    let outcome = ops::expand(&work_packed, tools, runner).map_err(|e| {
        cleanup(&stage_root);
        (format!("{e}"), code_for(&e))
    })?;
    let expanded = outcome.dir;

    // Stage 3: locate subtree (creating it from the shipped tox when absent)
    // + rewrite three DAT bodies.
    let mut created = false;
    let subtree = match find_subtree(&expanded).map_err(|e| {
        cleanup(&stage_root);
        (format!("bridge discovery failed: {e}"), e.code())
    })? {
        Some(found) => found,
        None => {
            created = true;
            create_subtree(&expanded, &outcome.toc, &stage_root, tools, runner)
                .inspect_err(|_| cleanup(&stage_root))?
        }
    };
    let subtree_rel = subtree
        .strip_prefix(&expanded)
        .map(Path::to_path_buf)
        .map_err(|_| {
            cleanup(&stage_root);
            (
                "bridge subtree escaped expanded staging root".to_string(),
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

    // Snapshot every expected entry after the intended bridge rewrite. The
    // private copy is the preservation oracle even if collapse mutates its
    // input or returns a success-shaped artifact that drops unrelated data.
    let bridge_entries: HashSet<String> = BRIDGE_DATS
        .iter()
        .map(|dat| subtree_rel.join(dat).to_string_lossy().replace('\\', "/"))
        .collect();
    let snapshot = stage_root.join("expected-manifest");
    let expected_entries = snapshot_manifest(&expanded, &outcome.toc, &snapshot)
        .inspect_err(|_| cleanup(&stage_root))?;

    // Stage 4a: drop the pre-rewrite packed copy so toecollapse can write it.
    std::fs::remove_file(&work_packed).map_err(|e| {
        cleanup(&stage_root);
        (format!("unstage removal: {e}"), "project.io_failed")
    })?;

    // Stage 4b: collapse working dir back over work.toe (same-name staging).
    let collapsed = ops::collapse(&expanded, &work_packed, tools, runner).map_err(|e| {
        cleanup(&stage_root);
        (format!("{e}"), code_for(&e))
    })?;

    // Stage 5: full verification — re-expand, compare the complete manifest
    // and every untouched entry, then validate all three final bridge bodies.
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
    let verify_outcome = ops::expand(&vpacked, tools, runner).map_err(|e| {
        cleanup(&stage_root);
        (format!("{e}"), code_for(&e))
    })?;
    let vdir = verify_outcome.dir;
    let vsubtree = find_subtree(&vdir)
        .map_err(|e| {
            cleanup(&stage_root);
            (
                format!("verification bridge discovery failed: {e}"),
                e.code(),
            )
        })?
        .ok_or_else(|| {
            cleanup(&stage_root);
            (
                "verification lost subtree".into(),
                "project.bridge_subtree_missing",
            )
        })?;
    if vsubtree != vdir.join(&subtree_rel) {
        cleanup(&stage_root);
        return Err((
            format!(
                "verification moved bridge subtree from {} to {}",
                subtree_rel.display(),
                vsubtree.display()
            ),
            "project.roundtrip_mismatch",
        ));
    }
    verify_preserved_manifest(
        &snapshot,
        &expected_entries,
        &vdir,
        &verify_outcome.toc,
        &bridge_entries,
    )
    .inspect_err(|_| cleanup(&stage_root))?;
    for (dat, src_name) in SOURCES {
        let got = std::fs::read(vsubtree.join(dat)).map_err(|e| {
            cleanup(&stage_root);
            (format!("verify read {dat}: {e}"), "project.io_failed")
        })?;
        let payload = sidecar::parse(&got);
        let want = embedded_source(src_name);
        if sidecar::normalize_lf(payload) != sidecar::normalize_lf(want) {
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
        // Palette store failures never originate here; keep the match total.
        PaletteStore { .. } => "project.io_failed",
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

    fn bridge(root: &Path, name: &str, dats: &[&str]) -> PathBuf {
        comp(root, name);
        let dir = root.join(name);
        for dat in dats {
            let payload = format!("# tdmcp_bridge {dat}\n");
            fs::write(
                dir.join(dat),
                tdmcp_projectio::sidecar::encode(payload.as_bytes()),
            )
            .unwrap();
        }
        dir
    }

    fn sibling(path: &Path, extension: &str) -> PathBuf {
        let mut value = path.as_os_str().to_os_string();
        value.push(extension);
        PathBuf::from(value)
    }

    fn tools_pair() -> OfficialTools {
        OfficialTools {
            expand: PathBuf::from("C:/fake/toeexpand.exe"),
            collapse: PathBuf::from("C:/fake/toecollapse.exe"),
        }
    }

    fn write_donor_expand(packed: &Path, root: &str) {
        let dir = sibling(packed, ".dir");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".build"), b"build 2025.32460\n").unwrap();
        let bridge = bridge(&dir, root, &BRIDGE_DATS);
        fs::write(bridge.join("keep.table"), b"donor-data\n").unwrap();
        fs::write(dir.join(format!("{root}.parm")), b"?\n").unwrap();
        fs::write(
            sibling(packed, ".toc"),
            format!(
                "# 4 0 0 0 1\n.build\n{root}.n\n{root}.parm\n{root}/bootstrap.text\n{root}/callbacks.text\n{root}/tdmcp_exec.text\n{root}/keep.table\n"
            ),
        )
        .unwrap();
    }

    fn write_existing_bridge_expand(
        packed: &Path,
        current_bridge: bool,
        keep: Option<&[u8]>,
        list_keep: bool,
    ) {
        let dir = sibling(packed, ".dir");
        comp(&dir, "project1");
        fs::write(dir.join(".build"), b"build target\n").unwrap();
        let bridge = bridge(&dir.join("project1"), "tdmcp_rs", &BRIDGE_DATS);
        for (dat, source) in SOURCES {
            let bytes = if current_bridge {
                tdmcp_projectio::sidecar::encode(&embedded_source(source))
            } else {
                b"# stale bridge payload\n".to_vec()
            };
            fs::write(bridge.join(dat), bytes).unwrap();
        }
        if let Some(bytes) = keep {
            fs::write(dir.join("project1").join("keep.bin"), bytes).unwrap();
        }
        let mut toc = String::from(
            ".build\nproject1.n\nproject1/tdmcp_rs.n\n\
             project1/tdmcp_rs/bootstrap.text\nproject1/tdmcp_rs/callbacks.text\n\
             project1/tdmcp_rs/tdmcp_exec.text\n",
        );
        if list_keep {
            toc.push_str("project1/keep.bin\n");
        }
        fs::write(sibling(packed, ".toc"), toc).unwrap();
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

    #[test]
    fn discovery_accepts_normal_and_renamed_complete_bridges() {
        for name in ["tdmcp_rs", "renamed_bridge"] {
            let tmp = tempfile::tempdir().unwrap();
            let expected = bridge(tmp.path(), name, &BRIDGE_DATS);
            fs::create_dir_all(tmp.path().join("unrelated")).unwrap();
            fs::write(tmp.path().join("unrelated").join("keep.bin"), b"keep").unwrap();
            assert_eq!(find_subtree(tmp.path()).unwrap(), Some(expected));
        }
    }

    #[test]
    fn discovery_ignores_unmarked_generic_dat_layouts() {
        let tmp = tempfile::tempdir().unwrap();
        comp(tmp.path(), "generic_scripts");
        for dat in BRIDGE_DATS {
            fs::write(
                tmp.path().join("generic_scripts").join(dat),
                b"generic payload\n",
            )
            .unwrap();
        }
        comp(tmp.path(), "one_generic_dat");
        fs::write(
            tmp.path().join("one_generic_dat").join("callbacks.text"),
            b"generic callback\n",
        )
        .unwrap();
        assert!(find_subtree(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn discovery_distinguishes_missing_partial_and_duplicate_layouts() {
        let missing = tempfile::tempdir().unwrap();
        comp(missing.path(), "ordinary_comp");
        assert!(find_subtree(missing.path()).unwrap().is_none());

        let partial = tempfile::tempdir().unwrap();
        bridge(partial.path(), "tdmcp_rs", &["bootstrap.text"]);
        let error = find_subtree(partial.path()).unwrap_err();
        assert!(matches!(error, BridgeLayoutError::Partial { .. }));
        assert!(error.to_string().contains("partial bridge"));

        let renamed_partial = tempfile::tempdir().unwrap();
        bridge(
            renamed_partial.path(),
            "renamed_partial",
            &["callbacks.text"],
        );
        let error = find_subtree(renamed_partial.path()).unwrap_err();
        assert!(matches!(error, BridgeLayoutError::Partial { .. }));

        let orphan = tempfile::tempdir().unwrap();
        bridge(orphan.path(), "renamed_bridge", &BRIDGE_DATS);
        fs::write(orphan.path().join("tdmcp_rs.n"), b"orphan node\n").unwrap();
        let error = find_subtree(orphan.path()).unwrap_err();
        assert!(matches!(error, BridgeLayoutError::Partial { .. }));

        let duplicate = tempfile::tempdir().unwrap();
        bridge(duplicate.path(), "tdmcp_rs", &BRIDGE_DATS);
        bridge(duplicate.path(), "renamed_bridge", &BRIDGE_DATS);
        let error = find_subtree(duplicate.path()).unwrap_err();
        assert!(matches!(error, BridgeLayoutError::Ambiguous { .. }));
        assert!(error.to_string().contains("multiple complete"));
    }

    #[test]
    fn renamed_donor_is_remapped_without_touching_unrelated_target_data() {
        let tmp = tempfile::tempdir().unwrap();
        let expanded = tmp.path().join("target.toe.dir");
        comp(&expanded, "project1");
        fs::write(expanded.join(".build"), b"build target\n").unwrap();
        fs::write(expanded.join("project1").join("keep.bin"), b"unchanged").unwrap();
        comp(&expanded.join("project1"), "generic_scripts");
        for dat in BRIDGE_DATS {
            fs::write(
                expanded.join("project1").join("generic_scripts").join(dat),
                b"generic payload\n",
            )
            .unwrap();
        }
        assert!(find_subtree(&expanded).unwrap().is_none());
        let target_toc = tmp.path().join("target.toe.toc");
        fs::write(
            &target_toc,
            b".build\nproject1.n\nproject1/keep.bin\nproject1/generic_scripts.n\n\
              project1/generic_scripts/bootstrap.text\nproject1/generic_scripts/callbacks.text\n\
              project1/generic_scripts/tdmcp_exec.text\n",
        )
        .unwrap();

        let donor_packed = tmp.path().join("donor.tox");
        write_donor_expand(&donor_packed, "renamed_donor_root");
        let installed = materialize_donor(
            &expanded,
            &target_toc,
            &sibling(&donor_packed, ".dir"),
            &sibling(&donor_packed, ".toc"),
        )
        .unwrap();

        assert_eq!(installed, expanded.join("project1").join("tdmcp_rs"));
        assert!(BRIDGE_DATS.iter().all(|dat| installed.join(dat).is_file()));
        assert!(expanded.join("project1").join("tdmcp_rs.n").is_file());
        assert!(installed.join("keep.table").is_file());
        assert!(!expanded
            .join("project1")
            .join("renamed_donor_root")
            .exists());
        assert_eq!(
            fs::read(expanded.join("project1").join("keep.bin")).unwrap(),
            b"unchanged"
        );
        for dat in BRIDGE_DATS {
            assert_eq!(
                fs::read(expanded.join("project1").join("generic_scripts").join(dat)).unwrap(),
                b"generic payload\n"
            );
        }
        let toc = fs::read_to_string(&target_toc).unwrap();
        assert!(toc.contains("project1/tdmcp_rs.n\n"));
        assert!(toc.contains("project1/tdmcp_rs/bootstrap.text\n"));
        assert!(!toc.contains("renamed_donor_root"));
    }

    #[test]
    fn donor_materialization_refuses_partial_destination_collision_before_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let expanded = tmp.path().join("target.toe.dir");
        comp(&expanded, "project1");
        fs::write(expanded.join(".build"), b"build target\n").unwrap();
        fs::write(expanded.join("project1").join("tdmcp_rs.n"), b"existing\n").unwrap();
        fs::write(expanded.join("project1").join("keep.bin"), b"unchanged").unwrap();
        let target_toc = tmp.path().join("target.toe.toc");
        let original_toc = b".build\nproject1.n\nproject1/tdmcp_rs.n\nproject1/keep.bin\n";
        fs::write(&target_toc, original_toc).unwrap();

        let donor_packed = tmp.path().join("donor.tox");
        write_donor_expand(&donor_packed, "another_root_name");
        let error = materialize_donor(
            &expanded,
            &target_toc,
            &sibling(&donor_packed, ".dir"),
            &sibling(&donor_packed, ".toc"),
        )
        .unwrap_err();

        assert_eq!(error.1, "project.bridge_subtree_missing");
        assert!(error.0.contains("refusing to overwrite"));
        assert!(!expanded.join("project1").join("tdmcp_rs").exists());
        assert_eq!(
            fs::read(expanded.join("project1").join("tdmcp_rs.n")).unwrap(),
            b"existing\n"
        );
        assert_eq!(fs::read(&target_toc).unwrap(), original_toc);
        assert_eq!(
            fs::read(expanded.join("project1").join("keep.bin")).unwrap(),
            b"unchanged"
        );
    }

    #[test]
    fn ensure_on_normal_current_target_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("current.toe");
        let original = [b'1', b'0', 0, 0, 0, 9];
        fs::write(&target, original).unwrap();
        let runner = tdmcp_projectio::runner::FakeOfficialRunner::default();
        runner.push_ok_with_effect(
            1,
            "",
            Box::new(|_, args| {
                let packed = PathBuf::from(&args[0]);
                let dir = sibling(&packed, ".dir");
                comp(&dir, "project1");
                fs::write(dir.join(".build"), b"build target\n").unwrap();
                let bridge = bridge(&dir.join("project1"), "tdmcp_rs", &BRIDGE_DATS);
                for (dat, source) in SOURCES {
                    fs::write(bridge.join(dat), embedded_source(source)).unwrap();
                }
                fs::write(
                    sibling(&packed, ".toc"),
                    b".build\nproject1.n\nproject1/tdmcp_rs.n\n\
                      project1/tdmcp_rs/bootstrap.text\nproject1/tdmcp_rs/callbacks.text\n\
                      project1/tdmcp_rs/tdmcp_exec.text\n",
                )
                .unwrap();
            }),
        );
        let params = ProjectInstallBridgeParams {
            target_path: target.to_string_lossy().into_owned(),
            strategy: Strategy::Ensure,
            backup: false,
        };
        let result = run_with_runner(&params, &tools_pair(), &runner).unwrap();
        assert_eq!(result["updated"], json!(false));
        assert_eq!(fs::read(&target).unwrap(), original);
        assert_eq!(
            runner
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            1
        );
    }

    #[test]
    fn successful_roundtrip_preserves_manifest_and_unrelated_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("project.toe");
        fs::write(&target, [b'1', b'0', 0, 0, 0, 9]).unwrap();
        let runner = tdmcp_projectio::runner::FakeOfficialRunner::default();
        runner.push_ok_with_effect(
            1,
            "",
            Box::new(|_, args| {
                write_existing_bridge_expand(
                    &PathBuf::from(&args[0]),
                    false,
                    Some(b"preserve me\n"),
                    true,
                );
            }),
        );
        runner.push_ok_with_effect(
            1,
            "",
            Box::new(|_, args| {
                fs::write(PathBuf::from(&args[0]), [b'1', b'0', 0, 0, 0, 7]).unwrap();
            }),
        );
        runner.push_ok_with_effect(
            1,
            "",
            Box::new(|_, args| {
                write_existing_bridge_expand(
                    &PathBuf::from(&args[0]),
                    true,
                    Some(b"preserve me\n"),
                    true,
                );
            }),
        );

        let params = ProjectInstallBridgeParams {
            target_path: target.to_string_lossy().into_owned(),
            strategy: Strategy::Force,
            backup: false,
        };
        let result = run_with_runner(&params, &tools_pair(), &runner).unwrap();
        assert_eq!(result["updated"], json!(true));
        assert_eq!(result["rewritten"].as_array().unwrap().len(), 3);
        assert_eq!(fs::read(&target).unwrap(), [b'1', b'0', 0, 0, 0, 7]);
    }

    #[test]
    fn ensure_verifies_all_final_bridge_bodies_not_only_rewritten_dat() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("partial-current.toe");
        let original = [b'1', b'0', 0, 0, 0, 9];
        fs::write(&target, original).unwrap();
        let runner = tdmcp_projectio::runner::FakeOfficialRunner::default();

        runner.push_ok_with_effect(
            1,
            "",
            Box::new(|_, args| {
                let packed = PathBuf::from(&args[0]);
                write_existing_bridge_expand(&packed, true, Some(b"preserve me\n"), true);
                fs::write(
                    sibling(&packed, ".dir")
                        .join("project1")
                        .join("tdmcp_rs")
                        .join("callbacks.text"),
                    b"# stale callbacks\n",
                )
                .unwrap();
            }),
        );
        runner.push_ok_with_effect(
            1,
            "",
            Box::new(|_, args| {
                fs::write(PathBuf::from(&args[0]), [b'1', b'0', 0, 0, 0, 7]).unwrap();
            }),
        );
        runner.push_ok_with_effect(
            1,
            "",
            Box::new(|_, args| {
                let packed = PathBuf::from(&args[0]);
                write_existing_bridge_expand(&packed, true, Some(b"preserve me\n"), true);
                fs::write(
                    sibling(&packed, ".dir")
                        .join("project1")
                        .join("tdmcp_rs")
                        .join("tdmcp_exec.text"),
                    b"# corrupted unchanged DAT\n",
                )
                .unwrap();
            }),
        );

        let params = ProjectInstallBridgeParams {
            target_path: target.to_string_lossy().into_owned(),
            strategy: Strategy::Ensure,
            backup: false,
        };
        let error = run_with_runner(&params, &tools_pair(), &runner).unwrap_err();
        assert_eq!(error.1, "project.roundtrip_mismatch");
        assert!(error.0.contains("round-trip mismatch in tdmcp_exec.text"));
        assert_eq!(fs::read(&target).unwrap(), original);
    }

    #[test]
    fn success_shaped_roundtrip_drop_or_corruption_never_replaces_target() {
        for corrupt_keep in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let target = tmp.path().join("project.toe");
            let original = [b'1', b'0', 0, 0, 0, 9];
            fs::write(&target, original).unwrap();
            let runner = tdmcp_projectio::runner::FakeOfficialRunner::default();

            runner.push_ok_with_effect(
                1,
                "",
                Box::new(|_, args| {
                    write_existing_bridge_expand(
                        &PathBuf::from(&args[0]),
                        false,
                        Some(b"preserve me\n"),
                        true,
                    );
                }),
            );
            runner.push_ok_with_effect(
                1,
                "",
                Box::new(|_, args| {
                    fs::write(PathBuf::from(&args[0]), [b'1', b'0', 0, 0, 0, 7]).unwrap();
                }),
            );
            runner.push_ok_with_effect(
                1,
                "",
                Box::new(move |_, args| {
                    if corrupt_keep {
                        write_existing_bridge_expand(
                            &PathBuf::from(&args[0]),
                            true,
                            Some(b"corrupted\n"),
                            true,
                        );
                    } else {
                        write_existing_bridge_expand(&PathBuf::from(&args[0]), true, None, false);
                    }
                }),
            );

            let params = ProjectInstallBridgeParams {
                target_path: target.to_string_lossy().into_owned(),
                strategy: Strategy::Force,
                backup: false,
            };
            let error = run_with_runner(&params, &tools_pair(), &runner).unwrap_err();
            assert_eq!(error.1, "project.roundtrip_mismatch");
            if corrupt_keep {
                assert!(error
                    .0
                    .contains("changed untouched entry project1/keep.bin"));
            } else {
                assert!(error.0.contains("changed the toc manifest"));
            }
            assert_eq!(fs::read(&target).unwrap(), original);
        }
    }

    #[test]
    fn failed_reexpansion_validation_never_replaces_packed_target() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("project.toe");
        let original = [b'1', b'0', 0, 0, 0, 9];
        fs::write(&target, original).unwrap();
        let runner = tdmcp_projectio::runner::FakeOfficialRunner::default();

        // Expand the target: it has no bridge and contains unrelated data.
        runner.push_ok_with_effect(
            1,
            "",
            Box::new(|_, args| {
                let packed = PathBuf::from(&args[0]);
                let dir = sibling(&packed, ".dir");
                comp(&dir, "project1");
                fs::write(dir.join(".build"), b"build target\n").unwrap();
                fs::write(dir.join("project1").join("keep.bin"), b"unchanged").unwrap();
                fs::write(
                    sibling(&packed, ".toc"),
                    b".build\nproject1.n\nproject1/keep.bin\n",
                )
                .unwrap();
            }),
        );
        // Expand the embedded donor with a deliberately noncanonical root.
        runner.push_ok_with_effect(
            1,
            "",
            Box::new(|_, args| {
                write_donor_expand(&PathBuf::from(&args[0]), "renamed_donor_root");
            }),
        );
        // Collapse writes a candidate packed artifact, still only in staging.
        runner.push_ok_with_effect(
            1,
            "",
            Box::new(|_, args| {
                let packed = PathBuf::from(&args[0]);
                assert_eq!(
                    fs::read(sibling(&packed, ".dir").join("project1").join("keep.bin")).unwrap(),
                    b"unchanged"
                );
                fs::write(packed, [b'1', b'0', 0, 0, 0, 7]).unwrap();
            }),
        );
        // Re-expand with the right layout but a wrong rewritten payload.
        runner.push_ok_with_effect(
            1,
            "",
            Box::new(|_, args| {
                let packed = PathBuf::from(&args[0]);
                let dir = sibling(&packed, ".dir");
                comp(&dir, "project1");
                fs::write(dir.join(".build"), b"build target\n").unwrap();
                let bridge = bridge(&dir.join("project1"), "tdmcp_rs", &BRIDGE_DATS);
                for dat in BRIDGE_DATS {
                    fs::write(bridge.join(dat), b"wrong payload\n").unwrap();
                }
                fs::write(
                    sibling(&packed, ".toc"),
                    b".build\nproject1.n\nproject1/tdmcp_rs.n\n\
                      project1/tdmcp_rs/bootstrap.text\nproject1/tdmcp_rs/callbacks.text\n\
                      project1/tdmcp_rs/tdmcp_exec.text\n",
                )
                .unwrap();
            }),
        );

        let params = ProjectInstallBridgeParams {
            target_path: target.to_string_lossy().into_owned(),
            strategy: Strategy::Force,
            backup: false,
        };
        let error = run_with_runner(&params, &tools_pair(), &runner).unwrap_err();
        assert_eq!(error.1, "project.roundtrip_mismatch");
        assert_eq!(fs::read(&target).unwrap(), original);
        assert!(!tmp.path().join("project.toe.dir").exists());
        assert!(!tmp.path().join("project.toe.toc").exists());
    }
}
