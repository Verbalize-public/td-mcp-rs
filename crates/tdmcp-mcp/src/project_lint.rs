//! `project_lint` — sanity checks over an unpacked project dir.
//!
//! Native checks are cheap text-grammar reads (no TD needed): strict-LF `.toc`
//! parse, toc↔filesystem consistency, duplicate entries, and tdmcp_rs bridge
//! DAT presence. Deep semantic checking is delegated to opendesigner's
//! `td-cli check --json` when discoverable on PATH (optional backend that
//! degrades to a reported-unavailable state, never an error).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tdmcp_projectio::resolve::OfficialTools;

/// Args for `project_lint`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectLintParams {
    /// Expand directory — or a packed `.toe`/`.tox`, which is auto-expanded
    /// into a private temp staging dir (cleaned up afterwards; the input file
    /// and its siblings are never touched).
    pub target_path: String,
}

/// One lint diagnostic.
#[derive(Debug)]
pub struct LintDiag {
    /// Stable code (`project.*` family).
    pub code: &'static str,
    /// `error` | `warning`.
    pub severity: &'static str,
    /// File the diagnostic refers to.
    pub path: String,
    /// Human message.
    pub message: String,
}

fn diag(
    code: &'static str,
    severity: &'static str,
    path: impl core::fmt::Display,
    message: impl core::fmt::Display,
) -> LintDiag {
    LintDiag {
        code,
        severity,
        path: path.to_string(),
        message: message.to_string(),
    }
}

/// Run all native checks against an expand dir.
pub fn lint_expand_dir(dir: &Path) -> Vec<LintDiag> {
    let mut out = Vec::new();
    let toc_path = tdmcp_projectio::toc::toc_path_for(dir);
    if !toc_path.exists() {
        out.push(diag(
            "project.toc_invalid",
            "error",
            toc_path.display(),
            "missing .toc",
        ));
        return out;
    }
    let entries = match tdmcp_projectio::toc::parse(&toc_path) {
        Ok(e) => e,
        Err(e) => {
            out.push(diag("project.toc_invalid", "error", toc_path.display(), e));
            return out;
        }
    };
    if let Err(e) = tdmcp_projectio::toc::validate_entries(dir, &entries) {
        out.push(diag("project.toc_escape", "error", toc_path.display(), e));
    }

    // Filesystem consistency.
    let mut listed: BTreeSet<String> = BTreeSet::new();
    for entry in &entries {
        listed.insert(entry.clone());
        if !tdmcp_projectio::toc::entry_path(dir, entry).exists() {
            out.push(diag(
                "project.toc_invalid",
                "error",
                entry,
                "listed in .toc but missing on disk",
            ));
        }
    }
    collect_files(dir, dir, &mut |rel| {
        if !listed.contains(rel) {
            out.push(diag(
                "project.toc_invalid",
                "warning",
                rel,
                "on disk but not in .toc",
            ));
        }
    });

    // Duplicate entries.
    let mut seen: BTreeSet<&String> = BTreeSet::new();
    for entry in entries.iter().filter(|e| e.ends_with(".n")) {
        if !seen.insert(entry) {
            out.push(diag(
                "project.toc_invalid",
                "error",
                entry,
                "duplicate entry",
            ));
        }
    }

    // Bridge subtree presence: the three DAT bodies must exist. Payload
    // identity vs embedded sources is checked by project_install_bridge.
    if let Some(bridge_dir) = crate::project_install::find_subtree(dir) {
        for dat in ["bootstrap.text", "callbacks.text", "tdmcp_exec.text"] {
            let p = bridge_dir.join(dat);
            if !p.exists() {
                out.push(diag(
                    "project.bridge_subtree_missing",
                    "error",
                    p.display(),
                    "bridge DAT missing",
                ));
            }
        }
    } else {
        out.push(diag(
            "project.bridge_subtree_missing",
            "error",
            dir.display(),
            "no tdmcp_rs COMP subtree",
        ));
    }
    out
}

fn collect_files(root: &Path, dir: &Path, f: &mut impl FnMut(&str)) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_files(root, &p, f);
            } else if let Ok(rel) = p.strip_prefix(root) {
                f(&rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

/// Stage a packed file into a private temp dir, expand it there via the
/// official tool, lint, then remove the staging dir.
///
/// `ops::expand` writes beside its input — so staging keeps the user's file
/// and its siblings untouched (spec §3.4: "auto-unpacks to a temp staging
/// dir"). Cleanup is best-effort; failure to clean is not reported as a lint
/// diagnostic.
fn lint_packed_staged(
    packed: &Path,
    tools: &OfficialTools,
    runner: &dyn tdmcp_projectio::runner::CommandRunner,
) -> Vec<LintDiag> {
    let fail = |code: &'static str, msg: String| vec![diag(code, "error", packed.display(), msg)];
    let stage = std::env::temp_dir().join(format!("tdmcp-lint-{}", uuid::Uuid::new_v4().simple()));
    if let Err(e) = std::fs::create_dir_all(&stage) {
        return fail("project.io_failed", format!("staging mkdir failed: {e}"));
    }
    let mut name = std::ffi::OsString::from("packed");
    if let Some(ext) = packed.extension() {
        name.push(".");
        name.push(ext);
    }
    let staged = stage.join(name);
    if let Err(e) = std::fs::copy(packed, &staged) {
        let _ = std::fs::remove_dir_all(&stage);
        return fail("project.io_failed", format!("staging copy failed: {e}"));
    }
    match tdmcp_projectio::ops::expand(&staged, tools, runner) {
        Ok(outcome) => {
            let diags = lint_expand_dir(&outcome.dir);
            let _ = std::fs::remove_dir_all(&stage);
            diags
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&stage);
            fail(crate::project_unpack::code_for(&e), format!("{e}"))
        }
    }
}

/// Entry point used by dispatch. `packed_tools` is resolved by the caller only
/// when the target sniffs as a packed file (plain dirs never pay the scan).
#[must_use]
pub fn run(
    params: &ProjectLintParams,
    td_cli_json: Option<Value>,
    packed_tools: Option<&OfficialTools>,
) -> Value {
    let target = PathBuf::from(&params.target_path);
    let looks_packed = !target.is_dir() && tdmcp_projectio::sniff::sniff_packed(&target).is_ok();
    let (diags, target_kind) = if looks_packed {
        match packed_tools {
            Some(tools) => {
                let runner = tdmcp_projectio::runner::ProcessRunner;
                (
                    lint_packed_staged(&target, tools, &runner),
                    "packed",
                )
            }
            None => (
                vec![diag(
                    "project.tool_missing",
                    "error",
                    target.display(),
                    "packed target requires official toeexpand — configure [official_tools] or install TouchDesigner",
                )],
                "packed",
            ),
        }
    } else {
        (lint_expand_dir(&target), "dir")
    };
    let errors = diags.iter().filter(|d| d.severity == "error").count();
    json!({
        "ok": errors == 0,
        "target": target.to_string_lossy(),
        "targetKind": target_kind,
        "diagnostics": diags.iter().map(|d| json!({
            "code": d.code, "severity": d.severity, "path": d.path, "message": d.message,
        })).collect::<Vec<_>>(),
        "counts": {"errors": errors, "warnings": diags.len() - errors},
        "backends": {"native": true, "tdCli": td_cli_json.is_some()},
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn flags_missing_bridge_and_toc_drift() {
        let dir = tempfile::tempdir().unwrap();
        let ed = dir.path().join("p.toe.dir");
        fs::create_dir_all(ed.join("project1")).unwrap();
        fs::write(
            tdmcp_projectio::toc::toc_path_for(&ed),
            b".build\nproject1.n\n",
        )
        .unwrap();
        fs::write(ed.join(".build"), b"version 099\n").unwrap();
        let diags = lint_expand_dir(&ed);
        assert!(diags
            .iter()
            .any(|d| d.code == "project.toc_invalid" && d.message.contains("missing on disk")));
        assert!(diags
            .iter()
            .any(|d| d.code == "project.bridge_subtree_missing"));
    }

    #[test]
    fn clean_minimal_project_has_no_errors() {
        let dir = tempfile::tempdir().unwrap();
        let ed = dir.path().join("p.toe.dir");
        fs::create_dir_all(ed.join("project1")).unwrap();
        fs::write(ed.join(".build"), b"version 099\n").unwrap();
        fs::write(ed.join("project1.n"), b"COMP:container\nend\n").unwrap();
        let bridge = ed.join("project1").join("tdmcp_rs");
        fs::create_dir_all(&bridge).unwrap();
        for dat in ["bootstrap.text", "callbacks.text", "tdmcp_exec.text"] {
            fs::write(bridge.join(dat), b"# x\n").unwrap();
        }
        fs::write(
            tdmcp_projectio::toc::toc_path_for(&ed),
            b".build\nproject1.n\n",
        )
        .unwrap();
        let diags = lint_expand_dir(&ed);
        assert!(!diags.iter().any(|d| d.severity == "error"), "{diags:?}");
    }

    fn sibling(p: &Path, ext: &str) -> PathBuf {
        let mut s = p.as_os_str().to_os_string();
        s.push(ext);
        PathBuf::from(s)
    }

    fn tools_pair() -> OfficialTools {
        OfficialTools {
            expand: PathBuf::from("C:/fake/toeexpand.exe"),
            collapse: PathBuf::from("C:/fake/toecollapse.exe"),
        }
    }

    /// Effect mimicking toeexpand beside its input, producing a tree that
    /// passes every native check (toc lists all files, bridge present).
    fn staged_project_effect() -> tdmcp_projectio::runner::RunnerEffect {
        Box::new(move |_program, args| {
            let packed = PathBuf::from(&args[0]);
            let dir = sibling(&packed, ".dir");
            fs::create_dir_all(dir.join("project1").join("tdmcp_rs")).unwrap();
            fs::write(dir.join(".build"), b"version 099\n").unwrap();
            fs::write(dir.join("project1.n"), b"COMP:container\nend\n").unwrap();
            for dat in ["bootstrap.text", "callbacks.text", "tdmcp_exec.text"] {
                fs::write(dir.join("project1").join("tdmcp_rs").join(dat), b"# x\n").unwrap();
            }
            fs::write(
                sibling(&packed, ".toc"),
                b".build\nproject1.n\nproject1/tdmcp_rs/bootstrap.text\n\
                  project1/tdmcp_rs/callbacks.text\nproject1/tdmcp_rs/tdmcp_exec.text\n",
            )
            .unwrap();
        })
    }

    fn packed_fixture(dir: &Path) -> PathBuf {
        let p = dir.join("proj.toe");
        fs::write(&p, [b'1', b'0', 0, 0, 0, 9]).unwrap();
        p
    }

    #[test]
    fn packed_staging_lints_without_touching_source_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let src = packed_fixture(dir.path());
        let runner = tdmcp_projectio::runner::FakeOfficialRunner::default();
        runner.push_ok_with_effect(1, "exit-1 noise", staged_project_effect());
        let diags = lint_packed_staged(&src, &tools_pair(), &runner);
        assert!(!diags.iter().any(|d| d.severity == "error"), "{diags:?}");
        // Staging never polluted the source location.
        assert!(!sibling(&src, ".dir").exists());
        assert!(!sibling(&src, ".toc").exists());
        // Exactly one tool call, against the staged copy — then cleaned up.
        let calls = runner
            .calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        assert_eq!(calls.len(), 1);
        let invoked = PathBuf::from(&calls[0].1[0]);
        assert_ne!(invoked, src);
        assert!(!invoked.parent().is_some_and(Path::exists));
    }

    #[test]
    fn packed_target_without_tools_reports_tool_missing() {
        let dir = tempfile::tempdir().unwrap();
        let src = packed_fixture(dir.path());
        let params = ProjectLintParams {
            target_path: src.to_string_lossy().into_owned(),
        };
        let v = run(&params, None, None);
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["targetKind"], "packed");
        assert!(v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "project.tool_missing"));
    }

    #[test]
    fn dir_target_reports_target_kind_dir() {
        let dir = tempfile::tempdir().unwrap();
        let ed = dir.path().join("p.toe.dir");
        fs::create_dir_all(ed.join("project1")).unwrap();
        let params = ProjectLintParams {
            target_path: ed.to_string_lossy().into_owned(),
        };
        let v = run(&params, None, None);
        assert_eq!(v["targetKind"], "dir");
    }
}
