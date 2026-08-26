//! Official-tool operations: `expand` / `collapse` with filesystem-evidence
//! success semantics. Exit codes are reported, never trusted (V2-0 law).

use std::path::{Path, PathBuf};

use crate::error::ProjectIoError;
use crate::resolve::OfficialTools;
use crate::runner::CommandRunner;
use crate::sniff::sniff_packed;
use crate::toc;

/// Result of a successful expand.
#[derive(Debug, Clone)]
pub struct ExpandOutcome {
    /// Published expand directory.
    pub dir: PathBuf,
    /// Sibling `.toc` path.
    pub toc: PathBuf,
    /// Parsed entry count.
    pub entries: usize,
    /// Child exit code (informational — often non-zero on success).
    pub exit_code: i32,
}

/// Result of a successful collapse.
#[derive(Debug, Clone)]
pub struct CollapseOutcome {
    /// Published packed file.
    pub out: PathBuf,
    /// Output size in bytes (0 would have failed evidence check).
    pub bytes: u64,
    /// Child exit code (informational).
    pub exit_code: i32,
}

/// Expand `packed` via `tools.expand`, judging success by artifacts only.
///
/// Writes beside the input (`{packed}.dir` + `{packed}.toc`) — caller owns
/// destination policy (overwrite/destDir rename) and staging cleanup.
///
/// # Errors
/// [`ProjectIoError::NotPackedFormat`] on bad magic, [`ProjectIoError::DestExists`]
/// when artifacts already exist, [`ProjectIoError::ExpandOutputMissing`] when
/// evidence fails regardless of exit code, [`ProjectIoError::TocInvalid`] /
/// [`ProjectIoError::TocEscape`] when produced artifacts fail validation
/// (partials cleaned).
pub fn expand(
    packed: &Path,
    tools: &OfficialTools,
    runner: &dyn CommandRunner,
) -> Result<ExpandOutcome, ProjectIoError> {
    sniff_packed(packed)?;
    let dir = sibling_with_ext(packed, ".dir");
    let toc_path = sibling_with_ext(packed, ".toc");
    if dir.exists() || toc_path.exists() {
        return Err(ProjectIoError::DestExists(packed.to_path_buf()));
    }
    let output = runner
        .run(&tools.expand, &[packed.as_os_str()])
        .map_err(|source| ProjectIoError::Fs {
            path: tools.expand.clone(),
            source,
        })?;
    // Filesystem-evidence gate (exit code ignored by design).
    let entries = match toc::parse(&toc_path) {
        Ok(entries) => entries,
        Err(ProjectIoError::Fs { path: _, .. }) => {
            // No toc at all == the canonical failure shape.
            cleanup_partials(packed);
            tracing::warn!(exit = output.code, stderr = %output.stderr, "toeexpand wrote no toc - cleaned partials");
            return Err(ProjectIoError::ExpandOutputMissing {
                packed: packed.to_path_buf(),
            });
        }
        Err(e @ ProjectIoError::TocInvalid { .. } | e @ ProjectIoError::TocEscape { .. }) => {
            cleanup_partials(packed);
            tracing::warn!(exit = output.code, stderr = %output.stderr, "toeexpand toc invalid - cleaned partials");
            return Err(e);
        }
        Err(e) => {
            cleanup_partials(packed);
            return Err(e);
        }
    };
    if !dir.is_dir() || entries.is_empty() {
        cleanup_partials(packed);
        return Err(ProjectIoError::ExpandOutputMissing {
            packed: packed.to_path_buf(),
        });
    }
    toc::validate_entries(dir.parent().unwrap_or(dir.as_path()), &entries)?;
    Ok(ExpandOutcome {
        dir,
        toc: toc_path,
        entries: entries.len(),
        exit_code: output.code,
    })
}

/// Collapse `src_dir` into `out` via `tools.collapse`, evidence-checked.
///
/// # Errors
/// [`ProjectIoError::SrcNotExpandDir`] / toc validation failures on the source;
/// [`ProjectIoError::DestExists`] when `out` exists; [`ProjectIoError::CollapseOutputMissing`]
/// when evidence fails (empty/missing output removed).
pub fn collapse(
    src_dir: &Path,
    out: &Path,
    tools: &OfficialTools,
    runner: &dyn CommandRunner,
) -> Result<CollapseOutcome, ProjectIoError> {
    toc::check_expand_dir(src_dir)?;
    if out.exists() {
        return Err(ProjectIoError::DestExists(out.to_path_buf()));
    }
    let output = runner
        .run(&tools.collapse, &[out.as_os_str()])
        .map_err(|source| ProjectIoError::Fs {
            path: tools.collapse.clone(),
            source,
        })?;
    let bytes = out
        .is_file()
        .then(|| std::fs::metadata(out).map(|m| m.len()).unwrap_or(0));
    match bytes {
        Some(n) if n > 0 => Ok(CollapseOutcome {
            out: out.to_path_buf(),
            bytes: n,
            exit_code: output.code,
        }),
        _ => {
            let _ = std::fs::remove_file(out);
            tracing::warn!(
                exit = output.code,
                stderr = %output.stderr,
                "toecollapse produced empty/missing output - removed partials"
            );
            Err(ProjectIoError::CollapseOutputMissing {
                out: out.to_path_buf(),
            })
        }
    }
}

fn sibling_with_ext(packed: &Path, ext: &str) -> PathBuf {
    let mut s = packed.as_os_str().to_os_string();
    s.push(ext);
    PathBuf::from(s)
}

fn cleanup_partials(packed: &Path) {
    let _ = std::fs::remove_dir_all(sibling_with_ext(packed, ".dir"));
    let _ = std::fs::remove_file(sibling_with_ext(packed, ".toc"));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use crate::runner::FakeOfficialRunner;
    use std::fs;

    fn tools_pair() -> OfficialTools {
        OfficialTools {
            expand: PathBuf::from("C:/fake/toeexpand.exe"),
            collapse: PathBuf::from("C:/fake/toecollapse.exe"),
        }
    }

    fn packed_fixture(dir: &Path) -> PathBuf {
        let p = dir.join("proj.toe");
        fs::write(&p, [b'1', b'0', 0, 0, 0, 9]).unwrap();
        p
    }

    /// Effect mimicking toeexpand: materialize .dir content + strict-LF .toc
    /// beside the input — including the observed exit-1-on-success behavior.
    fn expand_effect(entries: &'static str) -> crate::runner::RunnerEffect {
        Box::new(move |_program, args| {
            let packed = PathBuf::from(&args[0]);
            let dir = sibling_with_ext(&packed, ".dir");
            fs::create_dir_all(dir.join("project1")).unwrap();
            fs::write(dir.join(".build"), b"version 099\n").unwrap();
            fs::write(dir.join("project1.n"), b"COMP:container\nend\n").unwrap();
            fs::write(sibling_with_ext(&packed, ".toc"), entries).unwrap();
        })
    }

    #[test]
    fn expand_success_despite_nonzero_exit() {
        let dir = tempfile::tempdir().unwrap();
        let packed = packed_fixture(dir.path());
        let runner = FakeOfficialRunner::default();
        runner.push_ok_with_effect(1, "warning noise", expand_effect(".build\nproject1.n\n"));
        let out = expand(&packed, &tools_pair(), &runner).expect("evidence must win");
        assert_eq!(out.exit_code, 1);
        assert_eq!(out.entries, 2);
        assert!(out.dir.is_dir());
        assert!(out.toc.is_file());
    }

    #[test]
    fn expand_without_artifacts_fails_and_cleans_partials() {
        let dir = tempfile::tempdir().unwrap();
        let packed = packed_fixture(dir.path());
        let runner = FakeOfficialRunner::default();
        runner.push_ok(1, "", "");
        let res = expand(&packed, &tools_pair(), &runner);
        assert!(matches!(
            res,
            Err(ProjectIoError::ExpandOutputMissing { .. })
        ));
        assert!(!sibling_with_ext(&packed, ".dir").exists());
        assert!(!sibling_with_ext(&packed, ".toc").exists());
    }

    #[test]
    fn expand_rejects_existing_artifacts_without_invoking_tools() {
        let dir = tempfile::tempdir().unwrap();
        let packed = packed_fixture(dir.path());
        fs::create_dir_all(sibling_with_ext(&packed, ".dir")).unwrap();
        let runner = FakeOfficialRunner::default();
        assert!(matches!(
            expand(&packed, &tools_pair(), &runner),
            Err(ProjectIoError::DestExists(_))
        ));
        assert!(runner
            .calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty());
    }

    #[test]
    fn collapse_empty_output_is_failure_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("proj.toe.dir");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join(".build"), b"v\n").unwrap();
        fs::write(toc::toc_path_for(&src), b".build\n").unwrap();
        let out = dir.path().join("packed.toe");
        let runner = FakeOfficialRunner::default();
        runner.push_ok(0, "", "");
        let res = collapse(&src, &out, &tools_pair(), &runner);
        assert!(matches!(
            res,
            Err(ProjectIoError::CollapseOutputMissing { .. })
        ));
        assert!(!out.exists());
    }

    #[test]
    fn collapse_success_requires_positive_size() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("proj.toe.dir");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join(".build"), b"v\n").unwrap();
        fs::write(toc::toc_path_for(&src), b".build\n").unwrap();
        let out = dir.path().join("packed.toe");
        let runner = FakeOfficialRunner::default();
        runner.push_ok_with_effect(
            0,
            "",
            Box::new(|_program, args| {
                fs::write(PathBuf::from(&args[0]), [b'1', b'0']).unwrap();
            }),
        );
        let res = collapse(&src, &out, &tools_pair(), &runner);
        assert_eq!(res.expect("collapse ok").bytes, 2);
    }
}
