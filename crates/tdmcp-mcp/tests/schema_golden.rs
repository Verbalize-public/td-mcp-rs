//! Golden JSON Schema fixtures — derived `inputSchema` must not drift silently.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests"
)]

use std::path::PathBuf;

use serde_json::Value;
use tdmcp_mcp::{input_schema_for, ToolName};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/schemas")
}

fn assert_schema_matches(tool: &str) {
    let dir = fixtures_dir();
    let path = dir.join(format!("{tool}.json"));
    let name = ToolName::from_wire(tool).unwrap_or_else(|| panic!("unknown tool {tool}"));
    let actual = Value::Object(input_schema_for(name));
    if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
        std::fs::create_dir_all(&dir).expect("create fixtures dir");
        let pretty = serde_json::to_string_pretty(&actual).unwrap() + "\n";
        std::fs::write(&path, pretty).expect("write golden");
        return;
    }
    let expected_text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let expected: Value = serde_json::from_str(&expected_text).expect("parse fixture");
    assert_eq!(
        actual,
        expected,
        "schema drift for tool `{tool}`.\n\
         Set UPDATE_GOLDEN=1 to regenerate, or edit tests/fixtures/schemas/{tool}.json.\n\
         actual:\n{}\nexpected:\n{}",
        serde_json::to_string_pretty(&actual).unwrap(),
        serde_json::to_string_pretty(&expected).unwrap(),
    );
}

/// Every tool in `ToolName::ALL` must have a golden fixture. Looping the enum
/// (rather than one hand-written test per tool) is what keeps a newly added
/// tool from shipping unguarded -- `project_lint` had a fixture with no test,
/// and `spawn_td` / `kill_td` / `project_install_bridge` had neither.
#[test]
fn every_tool_schema_matches_golden() {
    for tool in ToolName::ALL {
        assert_schema_matches(tool.wire_str());
    }
}

#[test]
fn deny_unknown_fields_rejects_extra() {
    let err = serde_json::from_value::<tdmcp_mcp::ExecutePythonParams>(serde_json::json!({
        "pid": 1,
        "script": "result=1",
        "unknownField": true
    }));
    assert!(
        err.is_err(),
        "deny_unknown_fields should reject unknownField"
    );
}

#[test]
fn fleet_unknown_include_rejected() {
    let err = serde_json::from_value::<tdmcp_mcp::FleetParams>(serde_json::json!({
        "include": ["typo"]
    }));
    assert!(
        err.is_err(),
        "unknown fleet include enum variant must fail deserialize"
    );
}

#[test]
fn inspect_unknown_include_rejected() {
    let err = serde_json::from_value::<tdmcp_mcp::InspectParams>(serde_json::json!({
        "pid": 1,
        "paths": ["/project1"],
        "include": ["typo"]
    }));
    assert!(
        err.is_err(),
        "unknown inspect include enum variant must fail deserialize"
    );
}
