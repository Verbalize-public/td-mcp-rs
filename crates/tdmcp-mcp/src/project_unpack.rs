//! `project_unpack` — `.toe`/`.tox` → expand dir via official toeexpand.
//!
//! Offline tool (no pid; session-gate exempt). `toeexpand` writes beside its
//! input, so unpacking uses a private directory on the destination filesystem
//! and a copied packed input. The staged dir + toc are validated before prior
//! output is moved. Publishing still requires multiple filesystem renames: it
//! is not crash-atomic, but ordinary stash/publish failures attempt to roll
//! both artifacts back and retain named recovery paths if that rollback fails.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use tdmcp_config::ConfigFile;
use tdmcp_projectio::resolve::{self, OfficialTools};
use tdmcp_projectio::runner::CommandRunner;
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

/// Resolve official tools for offline operations (shared by unpack/pack/install).
///
/// # Errors
/// [`CodedError`] with `project.tool_missing` when nothing usable resolves.
pub fn resolve_official_tools(
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

pub(crate) fn code_for(e: &ProjectIoError) -> &'static str {
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
        // Palette store failures never originate here; keep the match total.
        ProjectIoError::PaletteStore { .. } => "project.io_failed",
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
    unpack_with(&params, &tools, &runner, &RealFsHook)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FsStep {
    CreateStageRoot,
    CreateWorkDir,
    CreateRecoveryDir,
    CopyPackedInput,
    StashDir,
    StashToc,
    PublishDir,
    PublishToc,
    RollbackPublishedDir,
    RollbackPublishedToc,
    RestoreDir,
    RestoreToc,
    CleanupStage,
}

/// Narrow injection seam for filesystem boundaries that participate in the
/// transaction. Expansion itself is injected through [`CommandRunner`].
trait FsStepHook: Sync {
    fn before(&self, _step: FsStep, _from: &Path, _to: Option<&Path>) -> std::io::Result<()> {
        Ok(())
    }
}

struct RealFsHook;
impl FsStepHook for RealFsHook {}

#[derive(Debug, Clone, Copy)]
enum ArtifactKind {
    Dir,
    Toc,
}

impl ArtifactKind {
    fn label(self) -> &'static str {
        match self {
            Self::Dir => "expand dir",
            Self::Toc => "toc",
        }
    }

    fn stash_step(self) -> FsStep {
        match self {
            Self::Dir => FsStep::StashDir,
            Self::Toc => FsStep::StashToc,
        }
    }

    fn publish_step(self) -> FsStep {
        match self {
            Self::Dir => FsStep::PublishDir,
            Self::Toc => FsStep::PublishToc,
        }
    }

    fn rollback_published_step(self) -> FsStep {
        match self {
            Self::Dir => FsStep::RollbackPublishedDir,
            Self::Toc => FsStep::RollbackPublishedToc,
        }
    }

    fn restore_step(self) -> FsStep {
        match self {
            Self::Dir => FsStep::RestoreDir,
            Self::Toc => FsStep::RestoreToc,
        }
    }
}

struct ArtifactTxn {
    kind: ArtifactKind,
    staged: PathBuf,
    dest: PathBuf,
    stash: PathBuf,
    prior_identity: Option<tdmcp_projectio::publish::FileIdentity>,
    new_identity: tdmcp_projectio::publish::FileIdentity,
    stashed_identity: Option<tdmcp_projectio::publish::FileIdentity>,
    stashed: bool,
    published: bool,
}

struct PublishFailure {
    message: String,
    code: &'static str,
    retain_stage: bool,
}

fn unpack_with(
    params: &ProjectUnpackParams,
    tools: &OfficialTools,
    runner: &dyn CommandRunner,
    hook: &dyn FsStepHook,
) -> Result<Value, CodedError> {
    let source = PathBuf::from(&params.source_path);
    if !source.is_file() {
        return Err(CodedError {
            message: format!("source not found: {}", source.display()),
            code: "project.source_not_found",
        });
    }

    let default_dest = sibling_ext(&source, ".dir");
    let dest_dir = params
        .dest_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or(default_dest);
    let dest_toc = tdmcp_projectio::toc::toc_path_for(&dest_dir);

    let initial_dir = identity_if_present(&dest_dir)
        .map_err(|e| io_failure("identify destination", &dest_dir, e))?;
    let initial_toc = identity_if_present(&dest_toc)
        .map_err(|e| io_failure("identify destination", &dest_toc, e))?;
    if params.overwrite == Overwrite::Fail && (initial_dir.is_some() || initial_toc.is_some()) {
        return Err(CodedError {
            message: format!("destination exists: {}", dest_dir.display()),
            code: "project.dest_exists",
        });
    }
    // Preserve the public format diagnostic before destination staging adds
    // new filesystem failure points. `ops::expand` repeats this check on the
    // copied input so the actual tool boundary remains evidence-gated.
    tdmcp_projectio::sniff::sniff_packed(&source).map_err(|e| CodedError {
        message: format!("{e}"),
        code: code_for(&e),
    })?;
    validate_destination_safety(&source, &dest_dir, &dest_toc)?;

    let dest_parent = nonempty_parent(&dest_dir);
    let stage_root = dest_parent.join(format!(".tdmcp-unpack-{}", uuid::Uuid::new_v4().simple()));
    let work_dir = stage_root.join("work");
    let recovery_dir = stage_root.join("recovery");

    create_dir_step(hook, FsStep::CreateStageRoot, &stage_root)
        .map_err(|e| io_failure("create private unpack stage", &stage_root, e))?;
    if let Err(e) = create_dir_step(hook, FsStep::CreateWorkDir, &work_dir) {
        return Err(cleanup_after_error(
            io_failure("create private unpack work dir", &work_dir, e),
            &stage_root,
            hook,
        ));
    }
    if let Err(e) = create_dir_step(hook, FsStep::CreateRecoveryDir, &recovery_dir) {
        return Err(cleanup_after_error(
            io_failure("create private unpack recovery dir", &recovery_dir, e),
            &stage_root,
            hook,
        ));
    }

    let packed_name = source
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("input.toe"));
    let staged_packed = work_dir.join(packed_name);
    if let Err(e) = copy_step(hook, &source, &staged_packed) {
        return Err(cleanup_after_error(
            io_failure("copy packed input into private stage", &staged_packed, e),
            &stage_root,
            hook,
        ));
    }

    let outcome = match tdmcp_projectio::ops::expand(&staged_packed, tools, runner) {
        Ok(outcome) => outcome,
        Err(e) => {
            let code = code_for(&e);
            return Err(cleanup_after_error(
                CodedError {
                    message: format!("{e}"),
                    code,
                },
                &stage_root,
                hook,
            ));
        }
    };

    if let Err(e) = reject_symlink_artifacts(&outcome.dir, &outcome.toc) {
        return Err(cleanup_after_error(
            CodedError {
                message: e,
                code: "project.io_failed",
            },
            &stage_root,
            hook,
        ));
    }
    let new_dir_identity = match tdmcp_projectio::publish::identity(&outcome.dir) {
        Ok(identity) => identity,
        Err(e) => {
            return Err(cleanup_after_error(
                io_failure("identify staged expand dir", &outcome.dir, e),
                &stage_root,
                hook,
            ));
        }
    };
    let new_toc_identity = match tdmcp_projectio::publish::identity(&outcome.toc) {
        Ok(identity) => identity,
        Err(e) => {
            return Err(cleanup_after_error(
                io_failure("identify staged toc", &outcome.toc, e),
                &stage_root,
                hook,
            ));
        }
    };

    // Recheck aliases after a potentially long tool run. Artifact identities
    // captured before expansion are reconciled inside the publish transaction;
    // a new or replaced destination is never adopted as prior output.
    if let Err(e) = validate_destination_safety(&source, &dest_dir, &dest_toc) {
        return Err(cleanup_after_error(e, &stage_root, hook));
    }
    if params.overwrite == Overwrite::Fail {
        let dir_now = path_present(&dest_dir).map_err(|e| {
            cleanup_after_error(
                io_failure("inspect destination", &dest_dir, e),
                &stage_root,
                hook,
            )
        })?;
        let toc_now = path_present(&dest_toc).map_err(|e| {
            cleanup_after_error(
                io_failure("inspect destination", &dest_toc, e),
                &stage_root,
                hook,
            )
        })?;
        if dir_now || toc_now {
            return Err(cleanup_after_error(
                CodedError {
                    message: format!("destination exists: {}", dest_dir.display()),
                    code: "project.dest_exists",
                },
                &stage_root,
                hook,
            ));
        }
    }

    let entries = outcome.entries;
    let exit_code = outcome.exit_code;
    let mut artifacts = [
        ArtifactTxn {
            kind: ArtifactKind::Dir,
            staged: outcome.dir,
            dest: dest_dir.clone(),
            stash: recovery_dir.join("previous-expand-dir"),
            prior_identity: initial_dir,
            new_identity: new_dir_identity,
            stashed_identity: None,
            stashed: false,
            published: false,
        },
        ArtifactTxn {
            kind: ArtifactKind::Toc,
            staged: outcome.toc,
            dest: dest_toc.clone(),
            stash: recovery_dir.join("previous.toc"),
            prior_identity: initial_toc,
            new_identity: new_toc_identity,
            stashed_identity: None,
            stashed: false,
            published: false,
        },
    ];

    if let Err(failure) = publish_transaction(&mut artifacts, params.overwrite, hook) {
        let err = CodedError {
            message: failure.message,
            code: failure.code,
        };
        return if failure.retain_stage {
            Err(err)
        } else {
            Err(cleanup_after_error(err, &stage_root, hook))
        };
    }

    if let Err(e) = cleanup_stage(&stage_root, hook) {
        return Err(CodedError {
            message: format!(
                "unpack published but private staging cleanup failed: {e}; retained recovery path: {}",
                stage_root.display()
            ),
            code: "project.io_failed",
        });
    }

    Ok(json!({
        "ok": true,
        "expandDir": dest_dir.to_string_lossy(),
        "tocPath": dest_toc.to_string_lossy(),
        "entries": entries,
        "exitCode": exit_code,
        "warnings": if exit_code == 0 {
            Vec::<Value>::new()
        } else {
            vec![json!({"note": "official tool exited non-zero but artifacts verified"})]
        },
    }))
}

fn publish_transaction(
    artifacts: &mut [ArtifactTxn; 2],
    overwrite: Overwrite,
    hook: &dyn FsStepHook,
) -> Result<(), PublishFailure> {
    for index in 0..artifacts.len() {
        let current = match identity_if_present(&artifacts[index].dest) {
            Ok(identity) => identity,
            Err(e) => {
                let cause = format!(
                    "identify {} before stash failed for {}: {e}",
                    artifacts[index].kind.label(),
                    artifacts[index].dest.display()
                );
                return Err(rollback_failure(cause, artifacts, hook));
            }
        };
        let expected = match (artifacts[index].prior_identity, current) {
            (None, None) => continue,
            (Some(expected), Some(current)) if expected == current => expected,
            _ => {
                let cause = format!(
                    "destination identity changed during expansion: {}",
                    artifacts[index].dest.display()
                );
                let mut failure = rollback_failure(cause, artifacts, hook);
                if overwrite == Overwrite::Fail {
                    failure.code = "project.dest_exists";
                }
                return Err(failure);
            }
        };

        let rename = rename_step(
            hook,
            artifacts[index].kind.stash_step(),
            &artifacts[index].dest,
            &artifacts[index].stash,
        );
        if let Err(e) = rename {
            let stash = artifacts[index].stash.clone();
            let cause = format!(
                "stash rename failed for {} -> {}: {e}",
                artifacts[index].dest.display(),
                stash.display()
            );
            let mut failure = rollback_failure(cause, artifacts, hook);
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                failure.retain_stage = true;
                failure.message = format!(
                    "{}; conflicting stash path retained: {}",
                    failure.message,
                    stash.display()
                );
            }
            return Err(failure);
        }
        artifacts[index].stashed = true;

        match tdmcp_projectio::publish::identity(&artifacts[index].stash) {
            Ok(moved) => {
                artifacts[index].stashed_identity = Some(moved);
                if moved != expected {
                    let cause = format!(
                        "stashed {} identity changed during rename at {}",
                        artifacts[index].kind.label(),
                        artifacts[index].stash.display()
                    );
                    return Err(rollback_failure(cause, artifacts, hook));
                }
            }
            Err(e) => {
                let cause = format!(
                    "identify stashed {} after rename at {}: {e}",
                    artifacts[index].kind.label(),
                    artifacts[index].stash.display()
                );
                return Err(rollback_failure(cause, artifacts, hook));
            }
        }
    }

    for index in 0..artifacts.len() {
        if let Err(e) = rename_step(
            hook,
            artifacts[index].kind.publish_step(),
            &artifacts[index].staged,
            &artifacts[index].dest,
        ) {
            let cause = format!(
                "publish rename failed for {} -> {}: {e}",
                artifacts[index].staged.display(),
                artifacts[index].dest.display()
            );
            let mut failure = rollback_failure(cause, artifacts, hook);
            if overwrite == Overwrite::Fail && e.kind() == std::io::ErrorKind::AlreadyExists {
                failure.code = "project.dest_exists";
            }
            return Err(failure);
        }
        artifacts[index].published = true;

        let published_matches = tdmcp_projectio::publish::identity(&artifacts[index].dest)
            .is_ok_and(|identity| identity == artifacts[index].new_identity);
        if !published_matches {
            let cause = format!(
                "published {} identity changed during rename at {}",
                artifacts[index].kind.label(),
                artifacts[index].dest.display()
            );
            let mut failure = rollback_failure(cause, artifacts, hook);
            failure.retain_stage = true;
            failure.message = format!(
                "{}; identity-conflicting artifacts retained: {}",
                failure.message,
                retained_recovery_paths(artifacts).join("; ")
            );
            return Err(failure);
        }
    }
    Ok(())
}

fn rollback_failure(
    cause: String,
    artifacts: &mut [ArtifactTxn; 2],
    hook: &dyn FsStepHook,
) -> PublishFailure {
    let mut rollback_errors = Vec::new();

    // First evacuate any newly-published artifact, so old artifacts have their
    // original names available again. Identity checks prevent moving a path a
    // concurrent writer replaced. Try every artifact even after an error.
    for artifact in artifacts.iter_mut().rev() {
        if !artifact.published {
            continue;
        }
        match identity_if_present(&artifact.dest) {
            Ok(Some(identity)) if identity == artifact.new_identity => {}
            Ok(Some(_)) => {
                rollback_errors.push(format!(
                    "new {} at {} was replaced concurrently; left in place",
                    artifact.kind.label(),
                    artifact.dest.display()
                ));
                continue;
            }
            Ok(None) => {
                rollback_errors.push(format!(
                    "new {} disappeared before rollback from {}",
                    artifact.kind.label(),
                    artifact.dest.display()
                ));
                continue;
            }
            Err(e) => {
                rollback_errors.push(format!(
                    "identify new {} before rollback at {}: {e}",
                    artifact.kind.label(),
                    artifact.dest.display()
                ));
                continue;
            }
        }
        match rename_step(
            hook,
            artifact.kind.rollback_published_step(),
            &artifact.dest,
            &artifact.staged,
        ) {
            Ok(()) => {
                artifact.published = false;
                match tdmcp_projectio::publish::identity(&artifact.staged) {
                    Ok(identity) if identity == artifact.new_identity => {}
                    Ok(_) => rollback_errors.push(format!(
                        "new {} identity changed while moving back to {}",
                        artifact.kind.label(),
                        artifact.staged.display()
                    )),
                    Err(e) => rollback_errors.push(format!(
                        "identify new {} after rollback at {}: {e}",
                        artifact.kind.label(),
                        artifact.staged.display()
                    )),
                }
            }
            Err(e) => rollback_errors.push(format!(
                "move new {} back from {} to {}: {e}",
                artifact.kind.label(),
                artifact.dest.display(),
                artifact.staged.display()
            )),
        }
    }

    for artifact in artifacts.iter_mut().rev() {
        if !artifact.stashed {
            continue;
        }
        let Some(expected) = artifact.stashed_identity else {
            rollback_errors.push(format!(
                "stashed {} has no recorded identity at {}",
                artifact.kind.label(),
                artifact.stash.display()
            ));
            continue;
        };
        match tdmcp_projectio::publish::identity(&artifact.stash) {
            Ok(identity) if identity == expected => {}
            Ok(_) => {
                rollback_errors.push(format!(
                    "previous {} at {} was replaced concurrently; retained",
                    artifact.kind.label(),
                    artifact.stash.display()
                ));
                continue;
            }
            Err(e) => {
                rollback_errors.push(format!(
                    "identify previous {} before restore at {}: {e}",
                    artifact.kind.label(),
                    artifact.stash.display()
                ));
                continue;
            }
        }
        match rename_step(
            hook,
            artifact.kind.restore_step(),
            &artifact.stash,
            &artifact.dest,
        ) {
            Ok(()) => {
                artifact.stashed = false;
                artifact.stashed_identity = None;
                match tdmcp_projectio::publish::identity(&artifact.dest) {
                    Ok(identity) if identity == expected => {}
                    Ok(_) => rollback_errors.push(format!(
                        "previous {} identity changed while restoring to {}",
                        artifact.kind.label(),
                        artifact.dest.display()
                    )),
                    Err(e) => rollback_errors.push(format!(
                        "identify previous {} after restore at {}: {e}",
                        artifact.kind.label(),
                        artifact.dest.display()
                    )),
                }
            }
            Err(e) => rollback_errors.push(format!(
                "restore previous {} from {} to {}: {e}",
                artifact.kind.label(),
                artifact.stash.display(),
                artifact.dest.display()
            )),
        }
    }

    if rollback_errors.is_empty() {
        return PublishFailure {
            message: format!("{cause}; destination state restored"),
            code: "project.io_failed",
            retain_stage: false,
        };
    }

    let retained = retained_recovery_paths(artifacts);
    PublishFailure {
        message: format!(
            "{cause}; rollback failed: {}; retained recovery paths: {}",
            rollback_errors.join("; "),
            retained.join("; ")
        ),
        code: "project.io_failed",
        retain_stage: true,
    }
}

fn retained_recovery_paths(artifacts: &[ArtifactTxn; 2]) -> Vec<String> {
    let mut retained = Vec::new();
    for artifact in artifacts {
        if artifact.stashed {
            retained.push(format!(
                "previous {}={}",
                artifact.kind.label(),
                artifact.stash.display()
            ));
        }
        let new_path = if artifact.published {
            &artifact.dest
        } else {
            &artifact.staged
        };
        retained.push(format!(
            "new {}={}",
            artifact.kind.label(),
            new_path.display()
        ));
    }
    retained
}

fn validate_destination_safety(
    source: &Path,
    dest_dir: &Path,
    dest_toc: &Path,
) -> Result<(), CodedError> {
    let source_target =
        std::fs::canonicalize(source).map_err(|e| io_failure("resolve source", source, e))?;
    let source_location =
        path_location(source).map_err(|e| io_failure("resolve source location", source, e))?;
    let dir_location = path_location(dest_dir)
        .map_err(|e| io_failure("resolve destination location", dest_dir, e))?;
    let toc_location = path_location(dest_toc)
        .map_err(|e| io_failure("resolve destination location", dest_toc, e))?;

    if source_location == dir_location || source_location == toc_location {
        return Err(alias_failure(source, dest_dir, dest_toc));
    }
    if dir_location == toc_location {
        return Err(alias_failure(source, dest_dir, dest_toc));
    }

    let dir_target = resolved_if_present(dest_dir)?;
    let toc_target = resolved_if_present(dest_toc)?;
    for target in [dir_target.as_ref(), toc_target.as_ref()]
        .into_iter()
        .flatten()
    {
        if target == &source_target || target == &source_location {
            return Err(alias_failure(source, dest_dir, dest_toc));
        }
        if std::fs::metadata(target).is_ok_and(|m| m.is_dir())
            && (source_target.starts_with(target) || source_location.starts_with(target))
        {
            return Err(alias_failure(source, dest_dir, dest_toc));
        }
    }
    if dir_target
        .as_ref()
        .zip(toc_target.as_ref())
        .is_some_and(|(dir, toc)| dir == toc)
    {
        return Err(alias_failure(source, dest_dir, dest_toc));
    }
    Ok(())
}

fn alias_failure(source: &Path, dest_dir: &Path, dest_toc: &Path) -> CodedError {
    CodedError {
        message: format!(
            "source/destination alias hazard rejected: source={}, expandDir={}, tocPath={}",
            source.display(),
            dest_dir.display(),
            dest_toc.display()
        ),
        code: "project.io_failed",
    }
}

fn resolved_if_present(path: &Path) -> Result<Option<PathBuf>, CodedError> {
    if !path_present(path).map_err(|e| io_failure("inspect destination", path, e))? {
        return Ok(None);
    }
    std::fs::canonicalize(path)
        .map(Some)
        .map_err(|e| io_failure("resolve destination alias", path, e))
}

/// Canonicalize the containing directory without following the final path
/// component. This catches a source path located inside a destination even if
/// the source itself is a symlink.
fn path_location(path: &Path) -> std::io::Result<PathBuf> {
    let parent = nonempty_parent(path);
    let parent = std::fs::canonicalize(parent)?;
    Ok(match path.file_name() {
        Some(name) => parent.join(name),
        None => parent,
    })
}

fn reject_symlink_artifacts(dir: &Path, toc: &Path) -> Result<(), String> {
    reject_symlinks_recursive(dir)?;
    let metadata = std::fs::symlink_metadata(toc)
        .map_err(|e| format!("inspect staged toc {}: {e}", toc.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "staged toc is a symlink and cannot be published safely: {}",
            toc.display()
        ));
    }
    Ok(())
}

fn reject_symlinks_recursive(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("inspect staged artifact {}: {e}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "staged artifact contains a symlink and cannot be published safely: {}",
            path.display()
        ));
    }
    if metadata.is_dir() {
        let entries = std::fs::read_dir(path)
            .map_err(|e| format!("scan staged artifact {}: {e}", path.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|e| format!("scan staged artifact {}: {e}", path.display()))?;
            reject_symlinks_recursive(&entry.path())?;
        }
    }
    Ok(())
}

fn identity_if_present(
    path: &Path,
) -> std::io::Result<Option<tdmcp_projectio::publish::FileIdentity>> {
    match tdmcp_projectio::publish::identity(path) {
        Ok(identity) => Ok(Some(identity)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

fn path_present(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn create_dir_step(hook: &dyn FsStepHook, step: FsStep, path: &Path) -> std::io::Result<()> {
    hook.before(step, path, None)?;
    std::fs::create_dir(path)
}

fn copy_step(hook: &dyn FsStepHook, from: &Path, to: &Path) -> std::io::Result<()> {
    hook.before(FsStep::CopyPackedInput, from, Some(to))?;
    std::fs::copy(from, to).map(|_| ())
}

fn rename_step(hook: &dyn FsStepHook, step: FsStep, from: &Path, to: &Path) -> std::io::Result<()> {
    hook.before(step, from, Some(to))?;
    tdmcp_projectio::publish::rename_noreplace(from, to)
}

fn cleanup_stage(stage_root: &Path, hook: &dyn FsStepHook) -> std::io::Result<()> {
    hook.before(FsStep::CleanupStage, stage_root, None)?;
    match std::fs::remove_dir_all(stage_root) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn cleanup_after_error(
    mut error: CodedError,
    stage_root: &Path,
    hook: &dyn FsStepHook,
) -> CodedError {
    if let Err(cleanup) = cleanup_stage(stage_root, hook) {
        error.message = format!(
            "{}; private staging cleanup failed: {cleanup}; retained recovery path: {}",
            error.message,
            stage_root.display()
        );
    }
    error
}

fn io_failure(action: &str, path: &Path, error: std::io::Error) -> CodedError {
    CodedError {
        message: format!("{action} for {}: {error}", path.display()),
        code: "project.io_failed",
    }
}

fn nonempty_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn sibling_ext(p: &Path, ext: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(ext);
    PathBuf::from(s)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::fs;
    use std::sync::Mutex;
    use tdmcp_projectio::runner::FakeOfficialRunner;

    #[derive(Debug)]
    enum HookAction {
        Fail {
            step: FsStep,
            kind: std::io::ErrorKind,
        },
        CreateFileAtTarget {
            step: FsStep,
            contents: &'static [u8],
        },
        CreateDirAtTarget {
            step: FsStep,
        },
        ReplaceSourceDir {
            step: FsStep,
            marker: &'static [u8],
        },
    }

    impl HookAction {
        fn step(&self) -> FsStep {
            match self {
                Self::Fail { step, .. }
                | Self::CreateFileAtTarget { step, .. }
                | Self::CreateDirAtTarget { step }
                | Self::ReplaceSourceDir { step, .. } => *step,
            }
        }
    }

    #[derive(Default)]
    struct ScriptedHook {
        actions: Mutex<VecDeque<HookAction>>,
        calls: Mutex<Vec<(FsStep, PathBuf, Option<PathBuf>)>>,
    }

    impl ScriptedHook {
        fn push(&self, action: HookAction) {
            self.actions
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push_back(action);
        }

        fn fail(&self, step: FsStep, kind: std::io::ErrorKind) {
            self.push(HookAction::Fail { step, kind });
        }

        fn create_file_at_target(&self, step: FsStep, contents: &'static [u8]) {
            self.push(HookAction::CreateFileAtTarget { step, contents });
        }

        fn create_dir_at_target(&self, step: FsStep) {
            self.push(HookAction::CreateDirAtTarget { step });
        }

        fn replace_source_dir(&self, step: FsStep, marker: &'static [u8]) {
            self.push(HookAction::ReplaceSourceDir { step, marker });
        }

        fn call_for(&self, step: FsStep) -> Option<(PathBuf, Option<PathBuf>)> {
            self.calls
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .iter()
                .find(|(called, _, _)| *called == step)
                .map(|(_, from, to)| (from.clone(), to.clone()))
        }
    }

    impl FsStepHook for ScriptedHook {
        fn before(&self, step: FsStep, from: &Path, to: Option<&Path>) -> std::io::Result<()> {
            self.calls.lock().unwrap_or_else(|p| p.into_inner()).push((
                step,
                from.to_path_buf(),
                to.map(Path::to_path_buf),
            ));
            let mut actions = self.actions.lock().unwrap_or_else(|p| p.into_inner());
            if !actions.front().is_some_and(|action| action.step() == step) {
                return Ok(());
            }
            match actions.pop_front().expect("front was present") {
                HookAction::Fail { kind, .. } => Err(std::io::Error::from(kind)),
                HookAction::CreateFileAtTarget { contents, .. } => {
                    fs::write(to.expect("target action requires destination"), contents)
                }
                HookAction::CreateDirAtTarget { .. } => {
                    fs::create_dir(to.expect("target action requires destination"))
                }
                HookAction::ReplaceSourceDir { marker, .. } => {
                    fs::remove_dir_all(from)?;
                    fs::create_dir(from)?;
                    fs::write(from.join("concurrent.marker"), marker)
                }
            }
        }
    }

    fn tools_pair() -> OfficialTools {
        OfficialTools {
            expand: PathBuf::from("C:/fake/toeexpand.exe"),
            collapse: PathBuf::from("C:/fake/toecollapse.exe"),
        }
    }

    fn packed_fixture(dir: &Path) -> PathBuf {
        let source = dir.join("project.toe");
        fs::write(&source, [b'1', b'0', 0, 0, 0, 9]).unwrap();
        source
    }

    fn expand_effect() -> tdmcp_projectio::runner::RunnerEffect {
        Box::new(|_program, args| {
            let packed = PathBuf::from(&args[0]);
            let dir = sibling_ext(&packed, ".dir");
            fs::create_dir_all(dir.join("project1")).unwrap();
            fs::write(dir.join(".build"), b"version 099\n").unwrap();
            fs::write(dir.join("project1.n"), b"COMP:container\nend\n").unwrap();
            fs::write(sibling_ext(&packed, ".toc"), b".build\nproject1.n\n").unwrap();
        })
    }

    fn replace_params(source: &Path, dest: &Path) -> ProjectUnpackParams {
        ProjectUnpackParams {
            source_path: source.to_string_lossy().into_owned(),
            dest_dir: Some(dest.to_string_lossy().into_owned()),
            overwrite: Overwrite::Replace,
            install_id: None,
        }
    }

    fn make_prior(dest: &Path) -> PathBuf {
        fs::create_dir_all(dest).unwrap();
        fs::write(dest.join("old.marker"), b"old-dir").unwrap();
        let toc = tdmcp_projectio::toc::toc_path_for(dest);
        fs::write(&toc, b"old.toc\n").unwrap();
        toc
    }

    #[test]
    fn expands_private_copy_then_replaces_both_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let source_dir = temp.path().join("source");
        let destination_dir = temp.path().join("destination");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&destination_dir).unwrap();
        let source = packed_fixture(&source_dir);
        let dest = destination_dir.join("published.toe.dir");
        let toc = make_prior(&dest);
        let source_bytes = fs::read(&source).unwrap();
        let runner = FakeOfficialRunner::default();
        runner.push_ok_with_effect(1, "warning", expand_effect());

        let result = unpack_with(
            &replace_params(&source, &dest),
            &tools_pair(),
            &runner,
            &ScriptedHook::default(),
        )
        .unwrap();

        assert_eq!(result["expandDir"], dest.to_string_lossy().as_ref());
        assert_eq!(result["tocPath"], toc.to_string_lossy().as_ref());
        assert_eq!(result["exitCode"], 1);
        assert!(dest.join("project1.n").is_file());
        assert_eq!(fs::read(&toc).unwrap(), b".build\nproject1.n\n");
        assert_eq!(fs::read(&source).unwrap(), source_bytes);
        assert!(!sibling_ext(&source, ".dir").exists());
        assert!(!sibling_ext(&source, ".toc").exists());
        let calls = runner.calls.lock().unwrap_or_else(|p| p.into_inner());
        let invoked = PathBuf::from(&calls[0].1[0]);
        assert_ne!(invoked, source);
        assert!(invoked.starts_with(&destination_dir));
        assert!(!invoked.exists());
    }

    #[test]
    fn invalid_packed_format_keeps_public_code_without_running_tool() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("invalid.toe");
        fs::write(&source, b"not a packed project").unwrap();
        let dest = temp.path().join("published.toe.dir");
        let runner = FakeOfficialRunner::default();

        let error = unpack_with(
            &replace_params(&source, &dest),
            &tools_pair(),
            &runner,
            &ScriptedHook::default(),
        )
        .unwrap_err();

        assert_eq!(error.code, "project.not_packed_format");
        assert!(source.is_file());
        assert!(runner
            .calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty());
    }

    #[test]
    fn expansion_failure_leaves_source_and_prior_output_intact() {
        let temp = tempfile::tempdir().unwrap();
        let source = packed_fixture(temp.path());
        let dest = temp.path().join("published.toe.dir");
        let toc = make_prior(&dest);
        let source_bytes = fs::read(&source).unwrap();
        let runner = FakeOfficialRunner::default();
        runner.push_err(std::io::Error::other("expand spawn failed"));

        let error = unpack_with(
            &replace_params(&source, &dest),
            &tools_pair(),
            &runner,
            &ScriptedHook::default(),
        )
        .unwrap_err();

        assert_eq!(error.code, "project.io_failed");
        assert_eq!(fs::read(&source).unwrap(), source_bytes);
        assert_eq!(fs::read(dest.join("old.marker")).unwrap(), b"old-dir");
        assert_eq!(fs::read(toc).unwrap(), b"old.toc\n");
        assert!(!sibling_ext(&source, ".dir").exists());
        assert!(!sibling_ext(&source, ".toc").exists());
    }

    #[test]
    fn success_shaped_expand_with_missing_toc_file_preserves_prior_pair() {
        let temp = tempfile::tempdir().unwrap();
        let source = packed_fixture(temp.path());
        let dest = temp.path().join("published.toe.dir");
        let toc = make_prior(&dest);
        let runner = FakeOfficialRunner::default();
        runner.push_ok_with_effect(
            0,
            "",
            Box::new(|_program, args| {
                let packed = PathBuf::from(&args[0]);
                let dir = sibling_ext(&packed, ".dir");
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join(".build"), b"version 099\n").unwrap();
                fs::write(sibling_ext(&packed, ".toc"), b".build\nmissing-project.n\n").unwrap();
            }),
        );

        let error = unpack_with(
            &replace_params(&source, &dest),
            &tools_pair(),
            &runner,
            &ScriptedHook::default(),
        )
        .unwrap_err();

        assert_eq!(error.code, "project.toc_invalid");
        assert!(error.message.contains("missing-project.n"));
        assert_eq!(fs::read(dest.join("old.marker")).unwrap(), b"old-dir");
        assert_eq!(fs::read(toc).unwrap(), b"old.toc\n");
        assert!(source.is_file());
    }

    #[test]
    fn second_stash_failure_rolls_first_artifact_back() {
        let temp = tempfile::tempdir().unwrap();
        let source = packed_fixture(temp.path());
        let dest = temp.path().join("published.toe.dir");
        let toc = make_prior(&dest);
        let runner = FakeOfficialRunner::default();
        runner.push_ok_with_effect(0, "", expand_effect());
        let hook = ScriptedHook::default();
        hook.fail(FsStep::StashToc, std::io::ErrorKind::PermissionDenied);

        let error = unpack_with(
            &replace_params(&source, &dest),
            &tools_pair(),
            &runner,
            &hook,
        )
        .unwrap_err();

        assert_eq!(error.code, "project.io_failed");
        assert!(error.message.contains("destination state restored"));
        assert_eq!(fs::read(dest.join("old.marker")).unwrap(), b"old-dir");
        assert_eq!(fs::read(toc).unwrap(), b"old.toc\n");
        assert!(source.is_file());
    }

    #[test]
    fn toc_publish_failure_rolls_new_dir_back_and_restores_both_old_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let source = packed_fixture(temp.path());
        let dest = temp.path().join("published.toe.dir");
        let toc = make_prior(&dest);
        let runner = FakeOfficialRunner::default();
        runner.push_ok_with_effect(0, "", expand_effect());
        let hook = ScriptedHook::default();
        hook.fail(FsStep::PublishToc, std::io::ErrorKind::PermissionDenied);

        let error = unpack_with(
            &replace_params(&source, &dest),
            &tools_pair(),
            &runner,
            &hook,
        )
        .unwrap_err();

        assert!(error.message.contains("destination state restored"));
        assert_eq!(fs::read(dest.join("old.marker")).unwrap(), b"old-dir");
        assert_eq!(fs::read(toc).unwrap(), b"old.toc\n");
        assert!(source.is_file());
    }

    #[test]
    fn concurrent_toc_created_inside_publish_hook_is_never_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let source = packed_fixture(temp.path());
        let dest = temp.path().join("published.toe.dir");
        let toc = make_prior(&dest);
        let runner = FakeOfficialRunner::default();
        runner.push_ok_with_effect(0, "", expand_effect());
        let hook = ScriptedHook::default();
        hook.create_file_at_target(FsStep::PublishToc, b"concurrent-toc");

        let error = unpack_with(
            &replace_params(&source, &dest),
            &tools_pair(),
            &runner,
            &hook,
        )
        .unwrap_err();

        let old_toc_stash = hook.call_for(FsStep::StashToc).unwrap().1.unwrap();
        assert!(error.message.contains("rollback failed"));
        assert!(error
            .message
            .contains(old_toc_stash.to_string_lossy().as_ref()));
        assert_eq!(fs::read(&toc).unwrap(), b"concurrent-toc");
        assert_eq!(fs::read(&old_toc_stash).unwrap(), b"old.toc\n");
        assert_eq!(fs::read(dest.join("old.marker")).unwrap(), b"old-dir");
    }

    #[test]
    fn concurrent_empty_dir_created_inside_publish_hook_is_never_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let source = packed_fixture(temp.path());
        let dest = temp.path().join("published.toe.dir");
        let toc = make_prior(&dest);
        let runner = FakeOfficialRunner::default();
        runner.push_ok_with_effect(0, "", expand_effect());
        let hook = ScriptedHook::default();
        hook.create_dir_at_target(FsStep::PublishDir);

        let error = unpack_with(
            &replace_params(&source, &dest),
            &tools_pair(),
            &runner,
            &hook,
        )
        .unwrap_err();

        let old_dir_stash = hook.call_for(FsStep::StashDir).unwrap().1.unwrap();
        assert!(error.message.contains("rollback failed"));
        assert!(error
            .message
            .contains(old_dir_stash.to_string_lossy().as_ref()));
        assert!(dest.is_dir());
        assert!(fs::read_dir(&dest).unwrap().next().is_none());
        assert_eq!(
            fs::read(old_dir_stash.join("old.marker")).unwrap(),
            b"old-dir"
        );
        assert_eq!(fs::read(toc).unwrap(), b"old.toc\n");
    }

    #[test]
    fn concurrent_dir_created_inside_restore_hook_is_never_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let source = packed_fixture(temp.path());
        let dest = temp.path().join("published.toe.dir");
        let toc = make_prior(&dest);
        let runner = FakeOfficialRunner::default();
        runner.push_ok_with_effect(0, "", expand_effect());
        let hook = ScriptedHook::default();
        hook.fail(FsStep::PublishToc, std::io::ErrorKind::PermissionDenied);
        hook.create_dir_at_target(FsStep::RestoreDir);

        let error = unpack_with(
            &replace_params(&source, &dest),
            &tools_pair(),
            &runner,
            &hook,
        )
        .unwrap_err();

        let old_dir_stash = hook.call_for(FsStep::StashDir).unwrap().1.unwrap();
        assert!(error.message.contains("rollback failed"));
        assert!(dest.is_dir());
        assert!(fs::read_dir(&dest).unwrap().next().is_none());
        assert_eq!(
            fs::read(old_dir_stash.join("old.marker")).unwrap(),
            b"old-dir"
        );
        assert_eq!(fs::read(toc).unwrap(), b"old.toc\n");
    }

    #[test]
    fn stash_source_identity_race_restores_concurrent_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let source = packed_fixture(temp.path());
        let dest = temp.path().join("published.toe.dir");
        let toc = make_prior(&dest);
        let runner = FakeOfficialRunner::default();
        runner.push_ok_with_effect(0, "", expand_effect());
        let hook = ScriptedHook::default();
        hook.replace_source_dir(FsStep::StashDir, b"concurrent-dir");

        let error = unpack_with(
            &replace_params(&source, &dest),
            &tools_pair(),
            &runner,
            &hook,
        )
        .unwrap_err();

        assert!(error.message.contains("identity changed"));
        assert_eq!(
            fs::read(dest.join("concurrent.marker")).unwrap(),
            b"concurrent-dir"
        );
        assert_eq!(fs::read(toc).unwrap(), b"old.toc\n");
    }

    #[test]
    fn stash_target_collision_is_retained_and_prior_output_is_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let source = packed_fixture(temp.path());
        let dest = temp.path().join("published.toe.dir");
        let toc = make_prior(&dest);
        let runner = FakeOfficialRunner::default();
        runner.push_ok_with_effect(0, "", expand_effect());
        let hook = ScriptedHook::default();
        hook.create_dir_at_target(FsStep::StashDir);

        let error = unpack_with(
            &replace_params(&source, &dest),
            &tools_pair(),
            &runner,
            &hook,
        )
        .unwrap_err();

        let conflict = hook.call_for(FsStep::StashDir).unwrap().1.unwrap();
        assert!(error.message.contains("conflicting stash path retained"));
        assert!(error.message.contains(conflict.to_string_lossy().as_ref()));
        assert!(conflict.is_dir());
        assert_eq!(fs::read(dest.join("old.marker")).unwrap(), b"old-dir");
        assert_eq!(fs::read(toc).unwrap(), b"old.toc\n");
    }

    #[test]
    fn exdev_publish_failure_restores_prior_output() {
        let temp = tempfile::tempdir().unwrap();
        let source = packed_fixture(temp.path());
        let dest = temp.path().join("published.toe.dir");
        let toc = make_prior(&dest);
        let runner = FakeOfficialRunner::default();
        runner.push_ok_with_effect(0, "", expand_effect());
        let hook = ScriptedHook::default();
        hook.fail(FsStep::PublishDir, std::io::ErrorKind::CrossesDevices);

        let error = unpack_with(
            &replace_params(&source, &dest),
            &tools_pair(),
            &runner,
            &hook,
        )
        .unwrap_err();

        assert_eq!(error.code, "project.io_failed");
        assert!(error.message.contains("destination state restored"));
        assert_eq!(fs::read(dest.join("old.marker")).unwrap(), b"old-dir");
        assert_eq!(fs::read(toc).unwrap(), b"old.toc\n");
        assert!(source.is_file());
    }

    #[test]
    fn rollback_failure_names_retained_old_and_new_paths() {
        let temp = tempfile::tempdir().unwrap();
        let source = packed_fixture(temp.path());
        let dest = temp.path().join("published.toe.dir");
        let toc = make_prior(&dest);
        let runner = FakeOfficialRunner::default();
        runner.push_ok_with_effect(0, "", expand_effect());
        let hook = ScriptedHook::default();
        hook.fail(FsStep::PublishToc, std::io::ErrorKind::PermissionDenied);
        hook.fail(FsStep::RestoreDir, std::io::ErrorKind::PermissionDenied);

        let error = unpack_with(
            &replace_params(&source, &dest),
            &tools_pair(),
            &runner,
            &hook,
        )
        .unwrap_err();

        let (retained_old_dir, _) = hook.call_for(FsStep::RestoreDir).unwrap();
        let (new_dir, _) = hook.call_for(FsStep::RollbackPublishedDir).unwrap();
        let staged_new_dir = hook
            .calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .find(|(step, _, _)| *step == FsStep::RollbackPublishedDir)
            .and_then(|(_, _, to)| to.clone())
            .unwrap();
        assert!(error.message.contains("rollback failed"));
        assert!(error.message.contains("retained recovery paths"));
        assert!(error
            .message
            .contains(retained_old_dir.to_string_lossy().as_ref()));
        assert!(error
            .message
            .contains(staged_new_dir.to_string_lossy().as_ref()));
        assert!(retained_old_dir.exists());
        assert!(!new_dir.exists());
        assert!(staged_new_dir.exists());
        assert_eq!(fs::read(toc).unwrap(), b"old.toc\n");
        assert!(source.is_file());
    }

    #[test]
    fn rejects_destination_containing_source_before_expansion() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("existing.toe.dir");
        fs::create_dir_all(&dest).unwrap();
        let source = packed_fixture(&dest);
        let runner = FakeOfficialRunner::default();

        let error = unpack_with(
            &replace_params(&source, &dest),
            &tools_pair(),
            &runner,
            &ScriptedHook::default(),
        )
        .unwrap_err();

        assert_eq!(error.code, "project.io_failed");
        assert!(error.message.contains("alias hazard"));
        assert!(source.is_file());
        assert!(runner
            .calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty());
    }

    #[test]
    fn fail_overwrite_rejects_dangling_symlink_collision() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let temp = tempfile::tempdir().unwrap();
            let source = packed_fixture(temp.path());
            let dest = temp.path().join("published.toe.dir");
            symlink(temp.path().join("missing-target"), &dest).unwrap();
            let runner = FakeOfficialRunner::default();
            let params = ProjectUnpackParams {
                source_path: source.to_string_lossy().into_owned(),
                dest_dir: Some(dest.to_string_lossy().into_owned()),
                overwrite: Overwrite::Fail,
                install_id: None,
            };

            let error =
                unpack_with(&params, &tools_pair(), &runner, &ScriptedHook::default()).unwrap_err();

            assert_eq!(error.code, "project.dest_exists");
            assert!(error.message.contains("destination exists"));
            assert!(runner
                .calls
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty());
        }
    }
}
