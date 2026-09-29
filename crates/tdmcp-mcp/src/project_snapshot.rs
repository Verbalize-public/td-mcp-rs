//! `project_snapshot` tool — save a project or a component, with a receipt.
//!
//! The arguments are strict (`deny_unknown_fields`). A misspelled `overwrite`
//! is the dangerous case here, not a cheap one: a dropped `overwrite` would
//! silently fall back to the default, and the default has to be the safe one.
//! `overwrite: "replace"` is the value that can destroy a file, so it must
//! never be the value a typo produces.
//!
//! # The requested path is not the artifact
//!
//! Measured on TouchDesigner 2025.32460, and this is the reason the tool takes
//! `target` as a required string rather than an optional path:
//!
//! * `COMP.save(path) -> str` returns the path it used, and saving twice to the
//!   same path returned that same path both times with a *different* digest —
//!   a silent in-place overwrite.
//! * `td.project.save(path) -> bool` returns a bool, and saving to an existing
//!   path wrote `name.toe` **and** `name.1.toe`, byte-identical, because the
//!   filename is incremented rather than overwritten.
//!
//! So a caller cannot know which file it caused, and the receipt reports the
//! path TouchDesigner produced alongside a digest of the bytes on disk. The
//! requested path is carried separately and is never the identity.
//!
//! # `overwrite` defaults to `fail`
//!
//! A save is a write to someone's disk. The default refuses an existing
//! destination before touching anything, so replacing a file is always a
//! deliberate request.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tdmcp_core::{OpPath, Pid};
use tdmcp_diagnostics::DiagnosticLevel;

/// A `project_snapshot` action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotAction {
    /// Save the whole project.
    #[default]
    Project,
    /// Save one COMP.
    Component,
    /// Report unsaved state; writes nothing.
    Status,
}

impl SnapshotAction {
    /// Wire string matching the bridge handler.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Component => "component",
            Self::Status => "status",
        }
    }
}

/// What to do when the destination already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SnapshotOverwrite {
    /// Refuse. The default, and the only safe one to arrive by accident.
    #[default]
    Fail,
    /// Replace or save alongside deliberately.
    Replace,
}

impl SnapshotOverwrite {
    /// Wire string matching the bridge handler.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Replace => "replace",
        }
    }
}

/// Args for `project_snapshot`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectSnapshotParams {
    /// Target pid.
    pub pid: Pid,
    /// Optional federated daemon id (omit for local / unique remote resolve).
    #[serde(default)]
    pub daemon_id: Option<String>,
    /// What to save, or `status` to report without writing. Defaults to
    /// `project`.
    #[serde(default)]
    pub action: SnapshotAction,
    /// Absolute output path. Required for both saving actions.
    ///
    /// Not optional because TouchDesigner's own default is an *incremented*
    /// name under `project.folder` — unpredictable, and therefore impossible
    /// to put in a receipt.
    #[serde(default)]
    pub target: Option<String>,
    /// The COMP to save, for `action: component`.
    #[serde(default)]
    pub path: Option<OpPath>,
    /// What to do when the destination exists. Defaults to `fail`.
    #[serde(default)]
    pub overwrite: SnapshotOverwrite,
    /// Optional context path for resolving a relative `path`.
    #[serde(default)]
    pub context_path: Option<OpPath>,
    /// Diagnostic payload size (`summary` omits raw traceback).
    #[serde(default)]
    pub diagnostic_level: DiagnosticLevel,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: serde_json::Value) -> ProjectSnapshotParams {
        serde_json::from_value(value).expect("valid params")
    }

    #[test]
    fn the_default_action_is_project_and_project_is_the_only_default_that_writes() {
        // `status` writes nothing, so it is the default for a caller that only
        // wanted to look. `project` is the default for an explicit save, and
        // the action is always stated either way.
        let params = parse(json!({"pid": 1, "target": "/tmp/a.toe"}));
        assert_eq!(params.action, SnapshotAction::Project);
        assert_eq!(params.action.as_str(), "project");
    }

    #[test]
    fn overwrite_defaults_to_fail_so_a_typo_never_replaces_a_file() {
        // `replace` destroys a file, so it must never be what a dropped or
        // misspelled option lands on.
        let params = parse(json!({"pid": 1, "target": "/tmp/a.toe"}));
        assert_eq!(params.overwrite, SnapshotOverwrite::Fail);
        assert_eq!(params.overwrite.as_str(), "fail");
    }

    #[test]
    fn a_misspelled_overwrite_is_refused_rather_than_defaulted() {
        // The dangerous version: silently becoming `fail` would be safe, but
        // silently becoming `replace` would not, and a caller cannot tell
        // which happened. Refusing is the only answer that is never wrong.
        for bad in ["clobber", "force", "yes", "true"] {
            assert!(
                serde_json::from_value::<ProjectSnapshotParams>(
                    json!({"pid": 1, "target": "/tmp/a.toe", "overwrite": bad})
                )
                .is_err(),
                "expected {bad:?} to be refused"
            );
        }
    }

    #[test]
    fn every_action_and_overwrite_round_trips_its_wire_spelling() {
        for (wire, expected) in [
            ("project", SnapshotAction::Project),
            ("component", SnapshotAction::Component),
            ("status", SnapshotAction::Status),
        ] {
            let params = parse(json!({"pid": 1, "action": wire}));
            assert_eq!(params.action, expected, "{wire}");
        }
        for (wire, expected) in [
            ("fail", SnapshotOverwrite::Fail),
            ("replace", SnapshotOverwrite::Replace),
        ] {
            let params = parse(json!({"pid": 1, "target": "/tmp/a.toe", "overwrite": wire}));
            assert_eq!(params.overwrite, expected, "{wire}");
        }
    }

    #[test]
    fn an_unknown_action_is_refused_by_the_type() {
        assert!(serde_json::from_value::<ProjectSnapshotParams>(
            json!({"pid": 1, "action": "pack"})
        )
        .is_err());
    }

    #[test]
    fn a_target_is_carried_when_given_and_absence_is_visible() {
        let with = parse(json!({"pid": 1, "target": "/tmp/a.toe"}));
        assert_eq!(with.target.as_deref(), Some("/tmp/a.toe"));
        // Absence is left to the bridge to refuse, which knows the action.
        let without = parse(json!({"pid": 1, "action": "status"}));
        assert!(without.target.is_none());
    }

    #[test]
    fn a_component_snapshot_carries_the_operator_path() {
        let params = parse(json!({
            "pid": 1, "action": "component",
            "path": "/project1/widget", "target": "/tmp/w.tox"
        }));
        assert_eq!(params.path, Some(OpPath::from("/project1/widget")));
        assert_eq!(params.action, SnapshotAction::Component);
    }
}
