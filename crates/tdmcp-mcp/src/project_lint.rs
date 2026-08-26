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

/// Args for `project_lint`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectLintParams {
    /// Expand directory (or packed `.toe`/`.tox`, which is expanded first).
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
        if !dir.join(entry.replace('/', "\\")).exists() {
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
    let bridge_dir = dir.join("project1").join("tdmcp_rs");
    if !bridge_dir.is_dir() {
        out.push(diag(
            "project.bridge_subtree_missing",
            "error",
            bridge_dir.display(),
            "no tdmcp_rs COMP subtree",
        ));
    } else {
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

/// Entry point used by dispatch.
#[must_use]
pub fn run(params: &ProjectLintParams, td_cli_json: Option<Value>) -> Value {
    let dir = PathBuf::from(&params.target_path);
    let diags = lint_expand_dir(&dir);
    let errors = diags.iter().filter(|d| d.severity == "error").count();
    json!({
        "ok": errors == 0,
        "target": dir.to_string_lossy(),
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
}
