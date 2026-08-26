//! `project_unpack` — `.toe`/`.tox` → expand dir via official toeexpand.
//!
//! Offline tool (no pid; session-gate exempt). Movement scheme (V2-0 law):
//! toeexpand writes beside its input, so we run it beside SOURCE, validate
//! artifacts (dir + strict-LF toc parse + escape check), then publish into the
//! destination — same-volume rename when possible. Replace-mode stashes prior
//! artifacts and restores them when expansion fails; any other failure cleans
//! partials beside the source.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use tdmcp_config::ConfigFile;
use tdmcp_projectio::resolve::{self, OfficialTools};
use tdmcp_projectio::ProjectIoError;

/// Args for `project_unpack`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectUnpackParams {
    /// Absolute path to the packed `.toe`/`.tox`.
    pub source_path: String,
    /// Destination expand-dir path. Default: `<source>.dir` beside the input.
    #[serde(default)]
    pub dest_dir: Option<String>,
    /// What to do when destination artifacts already exist.
    #[serde(default)]
    pub overwrite: Overwrite,
    /// Pin a specific install id from `td_installs` (default = newest usable).
    #[serde(default)]
    pub install_id: Option<String>,
}

/// Destination-collision policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Overwrite {
    /// Fail when destination exists.
    #[default]
    Fail,
    /// Stash existing artifacts aside, unpack fresh; restore on failure.
    Replace,
}

/// Tool-layer failure carrying its diagnostic code.
#[derive(Debug)]
pub struct CodedError {
    /// Human-readable message.
    pub message: String,
    /// Stable `tdmcp.*` code (already declared in codes.rs + catalog.yaml).
    pub code: &'static str,
}

fn load_config() -> Result<ConfigFile, CodedError> {
    let path = tdmcp_config::default_config_path();
    tdmcp_config::load(&path).map_err(|e| CodedError {
        message: format!("config load failed: {e}"),
        code: "project.io_failed",
    })
}

fn resolve_official_tools(
    cfg: &ConfigFile,
    install_id: Option<&str>,
) -> Result<OfficialTools, CodedError> {
    let mut src = resolve::ToolSource {
        expand: cfg.official_tools.expand_path.clone(),
        collapse: cfg.official_tools.collapse_path.clone(),
        td_exe: cfg.official_tools.td_exe.clone(),
    };
    if let Some(want) = install_id {
        src.expand = None;
        src.collapse = None;
        src.td_exe = None;
        'outer: for root in resolve::default_scan_roots(&resolve::std_env) {
            for exe in resolve::scan_install_exes(&root) {
                let info = resolve::inspect_install(&exe);
                let id_ok = info
                    .root
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|n| n == want);
                if id_ok {
                    src.td_exe = Some(exe);
                    break 'outer;
                }
            }
        }
    }
    resolve::resolve_tools(&src, &resolve::std_env).map_err(|e| CodedError {
        message: format!("{e}"),
        code: "project.tool_missing",
    })
}

fn code_for(e: &ProjectIoError) -> &'static str {
    match e {
        ProjectIoError::Fs { .. } => "project.io_failed",
        ProjectIoError::SourceNotFound(_) => "project.source_not_found",
        ProjectIoError::NotPackedFormat(_) => "project.not_packed_format",
        ProjectIoError::ToolMissing { .. } | ProjectIoError::ToolPairPartial => {
            "project.tool_missing"
        }
        ProjectIoError::ExpandOutputMissing { .. }
        | ProjectIoError::CollapseOutputMissing { .. } => "project.expand_failed",
        ProjectIoError::DestExists(_) => "project.dest_exists",
        ProjectIoError::SrcNotExpandDir { .. } => "project.src_not_expand_dir",
        ProjectIoError::TocEscape { .. } => "project.toc_escape",
        ProjectIoError::TocInvalid { .. } => "project.toc_invalid",
        ProjectIoError::RoundtripMismatch { .. } => "project.roundtrip_mismatch",
        ProjectIoError::BackupFailed { .. } => "project.backup_failed",
        ProjectIoError::BuildSkew { .. } => "project.build_skew",
    }
}

/// Execute an unpack.
pub fn run(args: Value) -> Result<Value, CodedError> {
    let params: ProjectUnpackParams = serde_json::from_value(args).map_err(|e| CodedError {
        message: e.to_string(),
        code: "args.invalid",
    })?;
    let source = PathBuf::from(&params.source_path);
    if !source.is_file() {
        return Err(CodedError {
            message: format!("source not found: {}", source.display()),
            code: "project.source_not_found",
        });
    }
    let cfg = load_config()?;
    let tools = resolve_official_tools(&cfg, params.install_id.as_deref())?;

    let runner = tdmcp_projectio::runner::ProcessRunner;
    let default_dest = sibling_ext(&source, ".dir");
    let dest_dir = params
        .dest_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| default_dest.clone());
    let dest_toc = tdmcp_projectio::toc::toc_path_for(&dest_dir);

    // Collision policy BEFORE running anything.
    let mut stash: Vec<(PathBuf, PathBuf)> = Vec::new();
    if dest_dir.exists() || dest_toc.exists() {
        match params.overwrite {
            Overwrite::Fail => {
                return Err(CodedError {
                    message: format!("destination exists: {}", dest_dir.display()),
                    code: "project.dest_exists",
                })
            }
            Overwrite::Replace => {
                for p in [&dest_dir, &dest_toc] {
                    if p.exists() {
                        let stash_path = sibling_uuid(p);
                        std::fs::rename(p, &stash_path).map_err(|e| CodedError {
                            message: format!("stash rename failed for {}: {e}", p.display()),
                            code: "project.io_failed",
                        })?;
                        stash.push((p.clone(), stash_path));
                    }
                }
            }
        }
    }

    let expand_result = tdmcp_projectio::ops::expand(&source, &tools, &runner);
    match expand_result {
        Ok(outcome) => {
            // Default destination IS what toeexpand wrote — keep canonical
            // layout (`<name>.toe.toc`). Custom destDir gets both files moved.
            let (final_dir, final_toc) = if dest_dir == outcome.dir {
                (outcome.dir.clone(), outcome.toc.clone())
            } else {
                std::fs::rename(&outcome.dir, &dest_dir)
                    .and_then(|_| std::fs::rename(&outcome.toc, &dest_toc))
                    .map_err(|e| CodedError {
                        message: format!("publish rename failed: {e}"),
                        code: "project.io_failed",
                    })?;
                (dest_dir.clone(), dest_toc.clone())
            };
            Ok(json!({
                "ok": true,
                "expandDir": final_dir.to_string_lossy(),
                "tocPath": final_toc.to_string_lossy(),
                "entries": outcome.entries,
                "exitCode": outcome.exit_code,
                "warnings": if outcome.exit_code == 0 {
                    Vec::<Value>::new()
                } else {
                    vec![json!({"note": "official tool exited non-zero but artifacts verified"})]
                },
            }))
        }
        Err(e) => {
            let code = code_for(&e);
            // Restore replace-mode stashes: previous state survives failures.
            for (orig, stash_path) in &stash {
                let _ = std::fs::rename(stash_path, orig);
            }
            Err(CodedError {
                message: format!("{e}"),
                code,
            })
        }
    }
}

fn sibling_ext(p: &Path, ext: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(ext);
    PathBuf::from(s)
}

fn sibling_uuid(p: &Path) -> PathBuf {
    p.with_extension(format!("stash-{}", uuid::Uuid::new_v4().simple()))
}
