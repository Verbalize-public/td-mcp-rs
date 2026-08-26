//! `project_pack` — expand dir → packed `.toe`/`.tox` via official toecollapse.
//!
//! Offline tool. Includes the build-skew guard (proposal §3.3): repacking with
//! tools of a different build than the source is how compat-dialog churn
//! starts, so it errors unless explicitly allowed.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use tdmcp_config::ConfigFile;
use tdmcp_projectio::resolve::{self, OfficialTools};
use tdmcp_projectio::{toc, ProjectIoError};

/// Args for `project_pack`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectPackParams {
    /// Expand directory produced by `project_unpack` (must contain `.build` + `.toc`).
    pub src_dir: String,
    /// Absolute output path for the packed file.
    pub out_path: String,
    /// What to do when the output file already exists.
    #[serde(default)]
    pub overwrite: Overwrite,
    /// Permit repacking with tools of a different build than the source.
    #[serde(default)]
    pub allow_build_skew: bool,
    /// Pin a specific install id from `td_installs`.
    #[serde(default)]
    pub install_id: Option<String>,
}

/// Output-collision policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Overwrite {
    /// Fail when output exists.
    #[default]
    Fail,
    /// Stash existing output aside; restore on failure.
    Replace,
}

/// Tool-layer failure carrying its diagnostic code.
#[derive(Debug)]
pub struct CodedError {
    /// Human-readable message.
    pub message: String,
    /// Stable `tdmcp.*` code.
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
) -> Result<(OfficialTools, PathBuf), CodedError> {
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
                if info
                    .root
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|n| n == want)
                {
                    src.td_exe = Some(exe);
                    break 'outer;
                }
            }
        }
    }
    let tools = resolve::resolve_tools(&src, &resolve::std_env).map_err(|e| CodedError {
        message: format!("{e}"),
        code: "project.tool_missing",
    })?;
    // Install root = two levels up from the collapse binary (bin -> versioned dir).
    let root = tools
        .collapse
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    Ok((tools, root))
}

fn code_for(e: &ProjectIoError) -> &'static str {
    match e {
        ProjectIoError::Fs { .. } => "project.io_failed",
        ProjectIoError::SourceNotFound(_) | ProjectIoError::NotPackedFormat(_) => {
            "project.not_packed_format"
        }
        ProjectIoError::ToolMissing { .. } | ProjectIoError::ToolPairPartial => {
            "project.tool_missing"
        }
        ProjectIoError::ExpandOutputMissing { .. }
        | ProjectIoError::CollapseOutputMissing { .. } => "project.collapse_failed",
        ProjectIoError::DestExists(_) => "project.dest_exists",
        ProjectIoError::SrcNotExpandDir { .. } => "project.src_not_expand_dir",
        ProjectIoError::TocEscape { .. } => "project.toc_escape",
        ProjectIoError::TocInvalid { .. } => "project.toc_invalid",
        ProjectIoError::RoundtripMismatch { .. } => "project.roundtrip_mismatch",
        ProjectIoError::BackupFailed { .. } => "project.backup_failed",
        ProjectIoError::BuildSkew { .. } => "project.build_skew",
    }
}

/// Execute a pack.
pub fn run(args: Value) -> Result<Value, CodedError> {
    let params: ProjectPackParams = serde_json::from_value(args).map_err(|e| CodedError {
        message: e.to_string(),
        code: "args.invalid",
    })?;
    let src_dir = PathBuf::from(&params.src_dir);
    let out_path = PathBuf::from(&params.out_path);
    let cfg = load_config()?;
    let (tools, install_root) = resolve_official_tools(&cfg, params.install_id.as_deref())?;

    // Build-skew guard: source `.build` vs selected install dir name.
    let source_build = toc::read_build(&src_dir);
    let tool_build = install_root
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .and_then(|n| n.strip_prefix("TouchDesigner."))
        .map(str::to_string);
    if !params.allow_build_skew {
        if let (Some(sb), Some(tb)) = (&source_build, &tool_build) {
            if sb != tb {
                return Err(CodedError {
                    message: format!("project built with {sb}, tools are {tb}"),
                    code: "project.build_skew",
                });
            }
        }
    }

    // Output collision policy BEFORE running (single file).
    let mut stash: Option<PathBuf> = None;
    if out_path.exists() {
        match params.overwrite {
            Overwrite::Fail => {
                return Err(CodedError {
                    message: format!("output exists: {}", out_path.display()),
                    code: "project.dest_exists",
                })
            }
            Overwrite::Replace => {
                let st = sibling_uuid(&out_path);
                std::fs::rename(&out_path, &st).map_err(|e| CodedError {
                    message: format!("stash rename failed: {e}"),
                    code: "project.io_failed",
                })?;
                stash = Some(st);
            }
        }
    }

    let runner = tdmcp_projectio::runner::ProcessRunner;
    match tdmcp_projectio::ops::collapse(&src_dir, &out_path, &tools, &runner) {
        Ok(outcome) => {
            if let Some(st) = &stash {
                let _ = std::fs::remove_file(st); // superseded; new pack verified
            }
            Ok(json!({
                "ok": true,
                "outPath": outcome.out.to_string_lossy(),
                "bytes": outcome.bytes,
                "exitCode": outcome.exit_code,
                "sourceBuild": source_build,
                "toolBuild": tool_build,
            }))
        }
        Err(e) => {
            let code = code_for(&e);
            if let Some(st) = &stash {
                let _ = std::fs::rename(st, &out_path); // restore prior output
            }
            Err(CodedError {
                message: format!("{e}"),
                code,
            })
        }
    }
}

fn sibling_uuid(p: &Path) -> PathBuf {
    p.with_extension(format!("stash-{}", uuid::Uuid::new_v4().simple()))
}
