//! `script_source` tool — named Python sources with immutable content refs.
//!
//! The arguments are strict (`deny_unknown_fields`) for the same reason
//! `execute_python`'s neighbours are: a misspelled `sourceId` would otherwise
//! be dropped and the call would fall through to `register`, which *stores*
//! something and runs nothing. That is a silent substitution in the opposite
//! direction — the caller asked to run a source and got a ref back.
//!
//! # Why a ref cannot go stale
//!
//! `sourceId` is the content address of the source it names (`src_` plus 16
//! bytes of SHA-256). The identity of a ref *is* its bytes, so a holder can
//! reconstruct the source from any peer without trusting this daemon, and
//! there is no "is my ref current?" question: a ref cannot name anything but
//! itself.
//!
//! What *can* go stale is the source's declared dependencies, and the bridge
//! reports those per run.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tdmcp_core::{OpPath, Pid};
use tdmcp_diagnostics::DiagnosticLevel;

/// A declared dependency on something outside the script.
///
/// A bare string is shorthand for an OP path, which is the common case and what
/// the bridge accepts. Two kinds are recognised, `op` and `module`; a wider
/// vocabulary would mean inventing dependency semantics this tool has no basis
/// for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ScriptDependency {
    /// Shorthand: an OP path.
    OpPath(String),
    /// Explicit kind and target.
    Detailed {
        /// `op` (default) or `module`.
        #[serde(default, rename = "kind")]
        kind: Option<String>,
        /// The path or module name.
        target: String,
    },
}

impl ScriptDependency {
    /// The declared kind, defaulting to `op`.
    #[must_use]
    pub fn kind(&self) -> &str {
        match self {
            Self::OpPath(_) => "op",
            Self::Detailed { kind, .. } => kind.as_deref().unwrap_or("op"),
        }
    }

    /// The path or module name.
    #[must_use]
    pub fn target(&self) -> &str {
        match self {
            Self::OpPath(path) => path,
            Self::Detailed { target, .. } => target,
        }
    }
}

/// A `script_source` action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ScriptSourceAction {
    /// Store a source and return an immutable ref. **Never executes it.**
    #[default]
    Register,
    /// Return one source, including its body.
    Describe,
    /// The roster of registered sources, without their bodies.
    List,
    /// Forget a source in the registry.
    Delete,
    /// Execute a registered source through the inline `execute_python` path.
    Run,
    /// Resolve a source's declared dependencies and report staleness.
    Check,
}

impl ScriptSourceAction {
    /// Wire string matching the bridge handler.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Describe => "describe",
            Self::List => "list",
            Self::Delete => "delete",
            Self::Run => "run",
            Self::Check => "check",
        }
    }
}

/// Args for `script_source`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScriptSourceParams {
    /// Target pid.
    pub pid: Pid,
    /// Optional federated daemon id (omit for local / unique remote resolve).
    #[serde(default)]
    pub daemon_id: Option<String>,
    /// What to do. Defaults to `register`, the only non-executing default.
    #[serde(default)]
    pub action: ScriptSourceAction,
    /// Source text, for `register`.
    #[serde(default)]
    pub source: Option<String>,
    /// Optional human name, recorded once and not part of the ref's identity.
    #[serde(default)]
    pub name: Option<String>,
    /// What the source needs, resolved and reported on every run.
    #[serde(default)]
    pub dependencies: Option<Vec<ScriptDependency>>,
    /// The content address returned by `register`. Required by every other
    /// action.
    #[serde(default)]
    pub source_id: Option<String>,
    /// Optional context path, forwarded to the run exactly as `execute_python`
    /// takes it.
    #[serde(default)]
    pub context_path: Option<OpPath>,
    /// When true (default), capture stdout/stderr during `run`.
    #[serde(default = "default_true")]
    pub include_logs: bool,
    /// Diagnostic payload size (`summary` omits raw traceback).
    #[serde(default)]
    pub diagnostic_level: DiagnosticLevel,
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: serde_json::Value) -> ScriptSourceParams {
        serde_json::from_value(value).expect("valid params")
    }

    #[test]
    fn the_default_action_is_register_and_register_is_not_run() {
        // The default must be the action that cannot touch the scene. A default
        // of `run` would execute whatever source happened to be supplied.
        let params = parse(json!({"pid": 1, "source": "result = 1"}));
        assert_eq!(params.action, ScriptSourceAction::Register);
    }

    #[test]
    fn every_action_round_trips_its_wire_spelling() {
        for (wire, expected) in [
            ("register", ScriptSourceAction::Register),
            ("describe", ScriptSourceAction::Describe),
            ("list", ScriptSourceAction::List),
            ("delete", ScriptSourceAction::Delete),
            ("run", ScriptSourceAction::Run),
            ("check", ScriptSourceAction::Check),
        ] {
            let params = parse(json!({"pid": 1, "action": wire}));
            assert_eq!(params.action, expected, "{wire}");
            assert_eq!(params.action.as_str(), wire);
        }
    }

    #[test]
    fn an_unknown_action_is_refused_by_the_type() {
        assert!(serde_json::from_value::<ScriptSourceParams>(
            json!({"pid": 1, "action": "compile"})
        )
        .is_err());
    }

    #[test]
    fn a_misspelled_source_id_is_refused_rather_than_dropped() {
        // Dropping it would fall through to `register`, which stores something
        // and runs nothing -- the opposite of what the caller asked for.
        assert!(serde_json::from_value::<ScriptSourceParams>(
            json!({"pid": 1, "action": "run", "sourceID": "src_x"})
        )
        .is_err());
    }

    #[test]
    fn a_dependency_keeps_its_kind_and_target() {
        let params = parse(json!({
            "pid": 1, "source": "result = 1",
            "dependencies": ["/project1/a", {"kind": "module", "target": "json"}]
        }));
        let deps = params.dependencies.expect("dependencies");
        // A bare string is an op path here too, so the two surfaces accept the
        // same shapes.
        assert_eq!(deps[0].kind(), "op");
        assert_eq!(deps[0].target(), "/project1/a");
        assert_eq!(deps[1].kind(), "module");
        assert_eq!(deps[1].target(), "json");
    }

    #[test]
    fn a_dependency_needs_a_target() {
        assert!(serde_json::from_value::<ScriptSourceParams>(json!({
            "pid": 1, "source": "result = 1",
            "dependencies": [{"kind": "module"}]
        }))
        .is_err());
    }

    #[test]
    fn run_options_match_the_inline_path() {
        // So a registered source is the same capability with a name: the same
        // options mean the same thing, forwarded to the same entry point.
        let params = parse(json!({
            "pid": 1, "action": "run", "sourceId": "src_x",
            "contextPath": "/project1", "includeLogs": false
        }));
        assert_eq!(params.source_id.as_deref(), Some("src_x"));
        assert_eq!(params.context_path, Some(OpPath::from("/project1")));
        assert!(!params.include_logs);
    }
}
