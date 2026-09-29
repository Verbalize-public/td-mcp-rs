//! `compare_nodes` tool — exact deltas from a fresh read against a baseline.
//!
//! Arguments are strict (`deny_unknown_fields`) for the same reason as
//! `sample_data`'s and `param_interface`'s: a misspelled `fields` would be
//! dropped, the comparison would run over a projection the caller never asked
//! for, and every unrequested field would be silently absent from the diff
//! rather than reported as changed.
//!
//! # What this tool is not
//!
//! It is not `inspect` called twice with the difference computed by the caller.
//! A diff is only actionable if the caller can show *when* each side was read,
//! and an inspect result carries no such stamp. So this tool reads the
//! projection on the bridge side and returns a stamped observation, and the
//! baseline is that observation, not a projection the caller assembled.
//!
//! # The baseline rules, stated up front
//!
//! * With no `baseline` and no `capture`, the call REFUSES
//!   (`baseline_unavailable`). An absent baseline is not an empty diff: an
//!   empty diff would tell the caller every field is unchanged when nothing was
//!   compared, and they would then write on top of a scene they never looked
//!   at.
//! * `capture: true` takes a fresh read and returns it for retention. It is
//!   opt-in precisely so a compare can never quietly become a capture and hand
//!   back `ok: true` to someone who lost their baseline.
//! * `atomic` and `exactlyOnce` are `false` in every response, and stay false
//!   in the schema documentation. Two reads and a comparison are three moments.
//!
//! # There is no counter shortcut
//!
//! A daemon mutation counter advances on dispatch, not on effect, so it is
//! never evidence that a scene is unchanged. This tool has no path that
//! consults one: every field reported as unchanged was re-read. If a field
//! cannot be read, it is reported unreadable rather than unchanged.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tdmcp_core::{OpPath, Pid};
use tdmcp_diagnostics::DiagnosticLevel;

/// `compare_nodes` arguments.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompareNodesParams {
    /// Target pid.
    pub pid: Pid,
    /// Operator paths to read, both sides.
    #[serde(default)]
    pub paths: Vec<OpPath>,
    /// Fields to project per path, e.g. `par.t`, `comment`, `wire.input.0`.
    ///
    /// Only these fields are compared. An unrequested field is not evidence of
    /// anything, so it is absent from the result rather than reported as
    /// unchanged.
    #[serde(default)]
    pub fields: Vec<String>,
    /// A previous `observation` from this tool, to compare against.
    ///
    /// Must be a stamped observation. A bare projection is refused: it cannot
    /// be shown to be a read of anything, so a delta against it would carry a
    /// freshness claim nothing backs.
    #[serde(default)]
    pub baseline: Option<serde_json::Value>,
    /// Take a fresh read and return it for retention, instead of comparing.
    #[serde(default)]
    pub capture: bool,
    /// Optional context path for resolving relative `paths`.
    #[serde(default)]
    pub context_path: Option<OpPath>,
    /// Optional federated daemon id (omit for local / unique remote resolve).
    #[serde(default)]
    pub daemon_id: Option<String>,
    /// Diagnostic payload size (`summary` omits raw traceback).
    #[serde(default)]
    pub diagnostic_level: DiagnosticLevel,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: serde_json::Value) -> CompareNodesParams {
        serde_json::from_value(value).expect("valid params")
    }

    #[test]
    fn comparing_is_the_default_and_capturing_is_opt_in() {
        // The default must be the mode that either compares or refuses, never
        // the one that silently hands back a fresh read.
        let params = parse(json!({"pid": 1, "paths": ["/project1/a"], "fields": ["par.t"]}));
        assert!(!params.capture);
        assert!(params.baseline.is_none());
    }

    #[test]
    fn a_misspelled_field_is_refused_rather_than_dropped() {
        // The whole reason these args are strict: a dropped `field` would
        // compare a projection the caller never asked for, and the unrequested
        // fields would be absent from the diff rather than reported changed.
        let err = serde_json::from_value::<CompareNodesParams>(json!({
            "pid": 1, "paths": ["/project1/a"], "field": ["par.t"]
        }));
        assert!(
            err.is_err(),
            "a misspelled field must not be silently dropped"
        );
    }

    #[test]
    fn a_misspelled_baseline_is_refused_rather_than_dropped() {
        let err = serde_json::from_value::<CompareNodesParams>(json!({
            "pid": 1, "paths": ["/project1/a"], "fields": ["par.t"], "basline": {}
        }));
        assert!(err.is_err());
    }

    #[test]
    fn an_omitted_projection_arrives_empty_rather_than_absent() {
        // Absent arrives as null through some clients; the bridge reports the
        // invalid-shape refusal. Either way it must not become a comparison
        // over everything.
        let params = parse(json!({"pid": 1}));
        assert!(params.paths.is_empty());
        assert!(params.fields.is_empty());
    }
}
