//! Derived JSON Schema helpers — one type → deserialize **and** `inputSchema`.

use rmcp::model::JsonObject;
use schemars::schema_for;
use serde_json::Value;

use crate::dialogs_tool::DialogsParams;
use crate::editor_context::EditorContextParams;
use crate::fleet::FleetParams;
use crate::lifecycle::{KillTdParams, SpawnTdParams};
use crate::project_install::ProjectInstallBridgeParams;
use crate::project_lint::ProjectLintParams;
use crate::project_pack::ProjectPackParams;
use crate::project_unpack::ProjectUnpackParams;
use crate::td_installs::TdInstallsParams;
use crate::tools::{
    ApiHelpParams, CaptureParams, ExecutePythonParams, InspectParams, MutateNodesParams, ToolName,
};

/// Empty object schema for tools with no parameters.
#[must_use]
pub fn empty_object_schema() -> JsonObject {
    let v = serde_json::json!({ "type": "object", "properties": {} });
    v.as_object().cloned().unwrap_or_default()
}

/// Derived `inputSchema` for a tool (SSOT = param type + schemars).
#[must_use]
pub fn input_schema_for(tool: ToolName) -> JsonObject {
    let schema = match tool {
        ToolName::Fleet => schema_value::<FleetParams>(),
        ToolName::ExecutePython => schema_value::<ExecutePythonParams>(),
        ToolName::Capture => schema_value::<CaptureParams>(),
        ToolName::Inspect => schema_value::<InspectParams>(),
        ToolName::MutateNodes => schema_value::<MutateNodesParams>(),
        ToolName::ApiHelp => schema_value::<ApiHelpParams>(),
        ToolName::EditorContext => schema_value::<EditorContextParams>(),
        ToolName::DescribeTools => Value::Object(empty_object_schema()),
        ToolName::TdInstalls => schema_value::<TdInstallsParams>(),
        ToolName::ProjectUnpack => schema_value::<ProjectUnpackParams>(),
        ToolName::ProjectPack => schema_value::<ProjectPackParams>(),
        ToolName::Dialogs => schema_value::<DialogsParams>(),
        ToolName::SpawnTd => schema_value::<SpawnTdParams>(),
        ToolName::KillTd => schema_value::<KillTdParams>(),
        ToolName::ProjectLint => schema_value::<ProjectLintParams>(),
        ToolName::ProjectInstallBridge => schema_value::<ProjectInstallBridgeParams>(),
    };
    // schemars may wrap with $schema / definitions — MCP wants a plain object schema.
    flatten_schema(schema)
}

fn schema_value<T: schemars::JsonSchema>() -> Value {
    let schema = schema_for!(T);
    serde_json::to_value(schema).unwrap_or_else(|_| serde_json::json!({ "type": "object" }))
}

/// Strip meta keys and inline `$defs` so the result is a flat JSON Schema object.
fn flatten_schema(mut schema: Value) -> JsonObject {
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("$schema");
        obj.remove("title");
        // Prefer inlined form; leave $defs if present (clients tolerate them).
    }
    schema.as_object().cloned().unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn fleet_schema_is_object() {
        let s = input_schema_for(ToolName::Fleet);
        assert_eq!(s.get("type").and_then(Value::as_str), Some("object"));
        assert!(s.contains_key("properties"));
    }

    #[test]
    fn execute_python_requires_pid_and_script() {
        let s = input_schema_for(ToolName::ExecutePython);
        let required = s
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let names: Vec<&str> = required.iter().filter_map(Value::as_str).collect();
        assert!(names.contains(&"pid"));
        assert!(names.contains(&"script"));
    }

    #[test]
    fn mutate_nodes_requires_pid_and_steps() {
        let s = input_schema_for(ToolName::MutateNodes);
        let required = s
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let names: Vec<&str> = required.iter().filter_map(Value::as_str).collect();
        assert!(names.contains(&"pid"));
        assert!(names.contains(&"steps"));
    }
}
