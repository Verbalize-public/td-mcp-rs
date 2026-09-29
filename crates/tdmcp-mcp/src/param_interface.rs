//! `param_interface` tool — export a COMP's control surface, or apply one.
//!
//! The arguments are strict (`deny_unknown_fields`) for the same reason as
//! `sample_data`'s: a misspelled `definitions` would otherwise be dropped, and
//! `apply` would answer "nothing to do" for a panel the caller asked to build.
//!
//! # Types
//!
//! The type vocabulary mirrors what the bridge measured on TouchDesigner
//! 2025.32460 and can actually create. `Unknown` is present only as an
//! *export* result — a parameter whose `is*` predicates matched nothing — and is
//! deliberately absent from the apply direction: there is no `appendUnknown`, so
//! applying one is reported as a skipped step rather than guessed at.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tdmcp_core::{OpPath, Pid};
use tdmcp_diagnostics::DiagnosticLevel;

/// A `param_interface` action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ParamInterfaceAction {
    /// The pages, parameters and types needed to drive a panel. The default,
    /// and the only action that cannot change anything.
    #[default]
    Export,
    /// Add the supplied definitions. Removes nothing.
    Apply,
}

impl ParamInterfaceAction {
    /// Wire string matching the bridge handler.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Export => "export",
            Self::Apply => "apply",
        }
    }
}

/// A parameter kind, as the bridge reports and creates it.
///
/// The names are the bridge's `append*` kinds. `unknown` is a value `export`
/// may report and `apply` will refuse with a reason — it exists so a consumer
/// can see "this type was not established" instead of receiving a confident
/// wrong one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ParameterKind {
    /// A floating-point number.
    Float,
    /// A whole number.
    Int,
    /// A boolean.
    Toggle,
    /// A momentary button.
    Pulse,
    /// A named-string menu.
    Menu,
    /// A text string.
    String,
    /// A sequence of keys.
    Sequence,
    /// An embedded Python module.
    Python,
    /// A folder of parameters.
    Folder,
    /// An operator path.
    Op,
    /// A momentary toggle.
    Momentary,
    /// Not established by this bridge; see the module docs.
    Unknown,
}

impl ParameterKind {
    /// Wire string as the bridge spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Float => "float",
            Self::Int => "int",
            Self::Toggle => "toggle",
            Self::Pulse => "pulse",
            Self::Menu => "menu",
            Self::String => "string",
            Self::Sequence => "sequence",
            Self::Python => "python",
            Self::Folder => "folder",
            Self::Op => "op",
            Self::Momentary => "momentary",
            Self::Unknown => "unknown",
        }
    }

    /// Whether the bridge can create this kind.
    ///
    /// `Unknown` cannot be created by anything, so an apply naming it is
    /// reported as a skipped step rather than approximated with a guess.
    #[must_use]
    pub const fn is_creatable(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

/// One parameter definition, as supplied to `apply` and as returned by `export`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParameterDefinition {
    /// Parameter name. Required.
    pub name: String,
    /// The kind. Required; `unknown` is refused by `apply`.
    #[serde(rename = "type")]
    pub kind: ParameterKind,
    /// Page to create it on. Defaults to `Controls`, as TouchDesigner does.
    #[serde(default)]
    pub page: Option<String>,
    /// Human-facing label. `None` leaves the label as the name.
    #[serde(default)]
    pub label: Option<String>,
    /// Default value.
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    /// Minimum, for a numeric parameter.
    #[serde(default)]
    pub min: Option<f64>,
    /// Maximum, for a numeric parameter.
    #[serde(default)]
    pub max: Option<f64>,
    /// Menu items, for a `menu`.
    #[serde(default)]
    pub menu_names: Option<Vec<String>>,
    /// Expression body, for a `python` parameter.
    ///
    /// This is the parameter's body, so a round trip that drops it yields a
    /// callback that exists and does nothing. Measured on TouchDesigner
    /// 2025.32460: assigning `expr` after creation is accepted, while an
    /// `expr=` keyword at creation is refused.
    #[serde(default)]
    pub expr: Option<String>,
}

/// Args for `param_interface`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParamInterfaceParams {
    /// Target pid.
    pub pid: Pid,
    /// COMP whose interface is exported or extended.
    pub path: OpPath,
    /// Optional federated daemon id (omit for local / unique remote resolve).
    #[serde(default)]
    pub daemon_id: Option<String>,
    /// Export the control surface, or apply one. Defaults to `export`.
    #[serde(default)]
    pub action: ParamInterfaceAction,
    /// Parameter definitions, for `apply`.
    #[serde(default)]
    pub definitions: Option<Vec<ParameterDefinition>>,
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

    fn parse(value: serde_json::Value) -> ParamInterfaceParams {
        serde_json::from_value(value).expect("valid params")
    }

    #[test]
    fn the_default_action_is_export_and_export_changes_nothing() {
        // The default must be the action that cannot alter a panel.
        let params = parse(json!({"pid": 1, "path": "/project1/comp"}));
        assert_eq!(params.action, ParamInterfaceAction::Export);
    }

    #[test]
    fn every_action_round_trips_its_wire_spelling() {
        for (wire, expected) in [
            ("export", ParamInterfaceAction::Export),
            ("apply", ParamInterfaceAction::Apply),
        ] {
            let params = parse(json!({"pid": 1, "path": "/p", "action": wire}));
            assert_eq!(params.action, expected, "{wire}");
            assert_eq!(params.action.as_str(), wire);
        }
    }

    #[test]
    fn an_unknown_action_is_refused_by_the_type() {
        assert!(serde_json::from_value::<ParamInterfaceParams>(
            json!({"pid": 1, "path": "/p", "action": "rewrite"})
        )
        .is_err());
    }

    #[test]
    fn a_misspelled_definitions_is_refused_rather_than_dropped() {
        // Dropping it would make `apply` answer "nothing to do" for a panel the
        // caller asked to build — a success that changed nothing.
        assert!(serde_json::from_value::<ParamInterfaceParams>(
            json!({"pid": 1, "path": "/p", "action": "apply", "definition": []})
        )
        .is_err());
    }

    #[test]
    fn every_creatable_kind_is_accepted_on_both_arms() {
        for kind in [
            "float",
            "int",
            "toggle",
            "pulse",
            "menu",
            "string",
            "sequence",
            "python",
            "folder",
            "op",
            "momentary",
        ] {
            let params = parse(json!({
                "pid": 1, "path": "/p", "action": "apply",
                "definitions": [{"name": "P", "type": kind}]
            }));
            let definition = params.definitions.expect("definitions")[0].clone();
            assert_eq!(definition.kind.as_str(), kind);
            assert!(definition.kind.is_creatable(), "{kind} must be creatable");
        }
    }

    #[test]
    fn unknown_is_reportable_but_never_creatable() {
        // It exists so an export can say "not established" without a
        // confident wrong answer, and so an apply naming it is refused.
        let kind = ParameterKind::Unknown;
        assert_eq!(kind.as_str(), "unknown");
        assert!(!kind.is_creatable());
    }

    #[test]
    fn a_definition_needs_a_name_and_a_type() {
        assert!(
            serde_json::from_value::<Vec<ParameterDefinition>>(json!([{"type": "float"}])).is_err()
        );
        assert!(
            serde_json::from_value::<Vec<ParameterDefinition>>(json!([{"name": "Speed"}])).is_err()
        );
    }

    #[test]
    fn a_numeric_bound_must_be_a_number() {
        // Typed rather than a JSON value, so a string bound is refused before
        // it reaches the bridge.
        assert!(serde_json::from_value::<Vec<ParameterDefinition>>(
            json!([{"name": "Speed", "type": "float", "min": "0"}])
        )
        .is_err());
        let ok = parse(json!({
            "pid": 1, "path": "/p", "action": "apply",
            "definitions": [{"name": "Speed", "type": "float", "min": 0.0, "max": 10.0}]
        }));
        let definition = ok.definitions.expect("definitions")[0].clone();
        assert_eq!(definition.min, Some(0.0));
        assert_eq!(definition.max, Some(10.0));
    }

    #[test]
    fn menu_items_and_a_page_travel_by_their_wire_names() {
        let params = parse(json!({
            "pid": 1, "path": "/p", "action": "apply",
            "definitions": [{
                "name": "Mode", "type": "menu", "page": "Advanced",
                "menuNames": ["a", "b"], "label": "Mode"
            }]
        }));
        let definition = params.definitions.expect("definitions")[0].clone();
        assert_eq!(definition.page.as_deref(), Some("Advanced"));
        assert_eq!(
            definition.menu_names,
            Some(vec!["a".to_owned(), "b".to_owned()])
        );
        assert_eq!(definition.label.as_deref(), Some("Mode"));
    }

    #[test]
    fn a_python_expression_travels_by_its_wire_name() {
        // A round trip that dropped it would produce a callback that exists and
        // does nothing, so the body is part of the definition, not an extra.
        let params = parse(json!({
            "pid": 1, "path": "/p", "action": "apply",
            "definitions": [{"name": "Callback", "type": "python", "expr": "print('hi')"}]
        }));
        let definition = params.definitions.expect("definitions")[0].clone();
        assert_eq!(definition.expr.as_deref(), Some("print('hi')"));
        assert_eq!(definition.kind, ParameterKind::Python);
    }

    #[test]
    fn an_expr_on_a_non_python_parameter_is_still_carried() {
        // Not refused here on purpose: the bridge owns the applicability
        // question, and a type-conditional refusal here would be a second rule
        // to keep in step with the first.
        let params = parse(json!({
            "pid": 1, "path": "/p", "action": "apply",
            "definitions": [{"name": "Speed", "type": "float", "expr": "1"}]
        }));
        let definition = params.definitions.expect("definitions")[0].clone();
        assert_eq!(definition.expr.as_deref(), Some("1"));
    }

    #[test]
    fn a_definition_with_an_unexpected_field_is_refused() {
        // A misspelled `minum` would otherwise be dropped and the parameter
        // created with a default range nobody asked for.
        assert!(serde_json::from_value::<Vec<ParameterDefinition>>(
            json!([{"name": "Speed", "type": "float", "minum": 0.0}])
        )
        .is_err());
    }
}
