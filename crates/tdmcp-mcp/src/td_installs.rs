//! `td_installs` — TouchDesigner installations on disk + official-tool
//! availability. Offline discovery tool (no pid; session-gate exempt).
//!
//! V2-0 law encoded here: a candidate counts only if the tool FILES exist —
//! stub installs list with `complete:false`, never silently usable.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use tdmcp_projectio::resolve;

/// Args for `td_installs` (none today; kept for forward-compatible schemas).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TdInstallsParams {}

/// Scan Program Files roots and build the installs response envelope.
#[must_use]
pub fn run(env: resolve::EnvLookup<'_>) -> Value {
    let mut rows = Vec::new();
    for root in resolve::default_scan_roots(env) {
        for exe in resolve::scan_install_exes(&root) {
            let info = resolve::inspect_install(&exe);
            let dir_name = info
                .root
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("unknown")
                .to_string();
            let version_label = dir_name
                .strip_prefix("TouchDesigner.")
                .unwrap_or(&dir_name)
                .to_string();
            let complete = info.toeexpand.is_some() && info.toecollapse.is_some();
            rows.push(json!({
                "installId": dir_name,
                "versionLabel": version_label,
                "rootPath": info.root.to_string_lossy(),
                "exePath": info.exe.to_string_lossy(),
                "tools": {
                    "toeexpand": info.toeexpand.as_ref().map(|p| p.to_string_lossy()),
                    "toecollapse": info.toecollapse.as_ref().map(|p| p.to_string_lossy()),
                    "python": info.python.as_ref().map(|p| p.to_string_lossy()),
                },
                "complete": complete,
            }));
        }
    }
    // Default = newest complete install (rows are already newest-first per root).
    if let Some(first_complete) = rows.iter_mut().find(|r| r["complete"] == json!(true)) {
        first_complete["default"] = json!(true);
    } else if let Some(last) = rows.last_mut() {
        last["default"] = json!(true);
    }
    json!({ "ok": true, "installs": rows })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use std::fs;

    fn fake_install(pf: &std::path::Path, version: &str, tools: bool) {
        let bin = pf
            .join("Derivative")
            .join(format!("TouchDesigner.{version}"))
            .join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("TouchDesigner.exe"), b"x").unwrap();
        if tools {
            fs::write(bin.join("toeexpand.exe"), b"e").unwrap();
            fs::write(bin.join("toecollapse.exe"), b"c").unwrap();
            fs::write(bin.join("python.exe"), b"p").unwrap();
        }
    }

    #[test]
    fn scan_marks_stub_incomplete_and_newest_complete_default() {
        let pf = tempfile::tempdir().unwrap();
        fake_install(pf.path(), "2025.32460", true);
        fake_install(pf.path(), "2025.33070", false); // stub
        let binding = [("ProgramFiles", pf.path().to_str().unwrap())];
        let envf = |name: &str| -> Option<String> {
            binding
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
        };
        let out = run(&envf);
        assert_eq!(out["ok"], true);
        let rows = out["installs"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        let stub = rows
            .iter()
            .find(|r| r["versionLabel"] == "2025.33070")
            .unwrap();
        assert_eq!(stub["complete"], false);
        assert_eq!(stub["tools"]["toeexpand"], Value::Null);
        let full = rows
            .iter()
            .find(|r| r["versionLabel"] == "2025.32460")
            .unwrap();
        assert_eq!(full["complete"], true);
        assert_eq!(full["default"], true);
    }

    #[test]
    fn empty_scan_yields_empty_installs() {
        let pf = tempfile::tempdir().unwrap();
        let binding = [("ProgramFiles", pf.path().to_str().unwrap())];
        let envf = move |name: &str| -> Option<String> {
            binding
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_string())
        };
        let out = run(&envf);
        assert!(out["installs"].as_array().unwrap().is_empty());
    }
}
