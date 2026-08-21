//! `editor_context` tool — live TD pane / selection snapshot.

use schemars::JsonSchema;
use serde::Deserialize;
use tdmcp_core::Pid;
use tdmcp_diagnostics::DiagnosticLevel;

/// Args for `editor_context`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditorContextParams {
    /// Target pid.
    pub pid: Pid,
    /// Optional federated daemon id (omit for local / unique remote resolve).
    #[serde(default)]
    pub daemon_id: Option<String>,
    /// Diagnostic payload size (`summary` omits raw traceback).
    #[serde(default)]
    pub diagnostic_level: DiagnosticLevel,
}
