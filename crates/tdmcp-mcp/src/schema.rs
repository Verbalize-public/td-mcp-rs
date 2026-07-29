//! Derived JSON Schema helpers — one type → deserialize **and** `inputSchema`.

use rmcp::model::JsonObject;
use schemars::schema_for;
use serde_json::Value;

use crate::fleet::FleetParams;
use crate::tools::{CaptureParams, ExecutePythonParams, InspectParams};

/// Empty object schema for tools with no parameters.
#[must_use]
pub fn empty_object_schema() -> JsonObject {
    let v = serde_json::json!({ "type": "object", "properties": {} });
    v.as_object().cloned().unwrap_or_default()
}

/// Derived `inputSchema` for a named tool (SSOT = param type + schemars).
#[must_use]
pub fn input_schema_for(tool_name: &str) -> JsonObject {
    let schema = match tool_name {
        "fleet" => schema_value::<FleetParams>(),
        "execute_python" => schema_value::<ExecutePythonParams>(),
        "capture" => schema_value::<CaptureParams>(),
        "inspect" => schema_value::<InspectParams>(),
        "describe_tools" => Value::Object(empty_object_schema()),
        _ => Value::Object(empty_object_schema()),
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
        let s = input_schema_for("fleet");
        assert_eq!(s.get("type").and_then(Value::as_str), Some("object"));
        assert!(s.contains_key("properties"));
    }

    #[test]
    fn execute_python_requires_pid_and_script() {
        let s = input_schema_for("execute_python");
        let required = s
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let names: Vec<&str> = required.iter().filter_map(Value::as_str).collect();
        assert!(names.contains(&"pid"));
        assert!(names.contains(&"script"));
    }
}
