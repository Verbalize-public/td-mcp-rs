//! Structured diagnostic envelope (rustc-inspired, agent-oriented).

use serde::{Deserialize, Serialize};

/// Severity ladder for diagnostic items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    /// Step failed / claim blocked.
    Error,
    /// Hint; did not fail alone.
    Lint,
    /// Context (partial apply, timeout honesty).
    Note,
    /// Curated playbook snippet.
    Help,
}

/// Coarse routing layer for agent loop re-entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLayer {
    /// Fleet / daemon identity and queues.
    Fleet,
    /// Structural inspect / TD cook errors.
    Structure,
    /// Perception / capture.
    Perception,
    /// Network mutate.
    Mutate,
    /// Script execution.
    Script,
}

/// Payload size for diagnostics (independent of structural `detailLevel`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    /// Codes, messages, capped lints, mitigation.
    #[default]
    Summary,
    /// Plus raw traceback / full TD dumps.
    Detailed,
}

/// Exact tool + step location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSpan {
    /// Tool name (e.g. `mutate_nodes`, `execute_python`).
    pub tool: String,
    /// Mutation index when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mutation_index: Option<u32>,
    /// Field name when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Script line when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Script column when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    /// Source snippet when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

/// Structured context for an item.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticContext {
    /// Operator path that failed resolution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op_path: Option<String>,
    /// Resolution base.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_path: Option<String>,
    /// Process id when relevant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Captured stdout/stderr from execute_python (when includeLogs was on).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs: Option<String>,
}

/// Suggested fix (never auto-applied).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    /// Suggested absolute operator path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op_path: Option<String>,
    /// Suggested replacement text (e.g. typo fix).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replace: Option<String>,
}

/// Nested lint under an error item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LintItem {
    /// Always `lint` for nested items.
    pub severity: DiagnosticSeverity,
    /// Stable code (e.g. `tdmcp.op.similar_name`).
    pub code: String,
    /// Human-readable message.
    pub message: String,
    /// Confidence of the hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    /// Optional suggestion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<Suggestion>,
}

/// Curated reference for agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    /// Kind: `doc`, `corpus`, `api_help`.
    pub kind: String,
    /// Identifier or query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Query string for api_help.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

/// One diagnostic item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticItem {
    /// Severity.
    pub severity: DiagnosticSeverity,
    /// Stable code.
    pub code: String,
    /// Coarse layer.
    pub layer: DiagnosticLayer,
    /// Human message.
    pub message: String,
    /// Exact location.
    pub span: DiagnosticSpan,
    /// Structured context.
    #[serde(default, skip_serializing_if = "DiagnosticContext::is_empty")]
    pub context: DiagnosticContext,
    /// Nested lints (capped).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lints: Vec<LintItem>,
    /// Mitigation steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mitigation: Vec<String>,
    /// References.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<Reference>,
    /// Raw traceback (detailed only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_traceback: Option<String>,
}

impl DiagnosticContext {
    fn is_empty(&self) -> bool {
        self.op_path.is_none()
            && self.context_path.is_none()
            && self.pid.is_none()
            && self.logs.is_none()
    }
}

/// Top-level diagnostics block on a tool response.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    /// One-line human summary.
    pub summary: String,
    /// Items.
    pub items: Vec<DiagnosticItem>,
}

impl Diagnostics {
    /// Empty diagnostics with a summary.
    #[must_use]
    pub fn empty(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            items: Vec::new(),
        }
    }

    /// True if any item has severity `error`.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.items
            .iter()
            .any(|i| i.severity == DiagnosticSeverity::Error)
    }

    /// Count errors and lints for a default summary line.
    #[must_use]
    pub fn recount_summary(&self) -> String {
        let errors = self
            .items
            .iter()
            .filter(|i| i.severity == DiagnosticSeverity::Error)
            .count();
        let lints = self.items.iter().map(|i| i.lints.len()).sum::<usize>()
            + self
                .items
                .iter()
                .filter(|i| i.severity == DiagnosticSeverity::Lint)
                .count();
        format!("{errors} errors, {lints} lints")
    }
}
