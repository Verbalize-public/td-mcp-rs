//! Cross-language parity: Rust `BridgeMethod` wire strings match the shared fixture.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests"
)]

use std::path::PathBuf;

use tdmcp_core::BridgeMethod;

#[test]
fn wire_strings_match_shared_fixture() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bridge/fixtures/bridge_methods.json");
    let text = std::fs::read_to_string(&fixture)
        .unwrap_or_else(|e| panic!("read {}: {e}", fixture.display()));
    let expected: Vec<String> = serde_json::from_str(&text).expect("parse fixture");
    let mut actual: Vec<String> = BridgeMethod::ALL
        .iter()
        .map(|m| m.wire_str().to_owned())
        .collect();
    actual.sort();
    let mut expected = expected;
    expected.sort();
    assert_eq!(
        actual, expected,
        "BridgeMethod::ALL wire strings drifted from bridge/fixtures/bridge_methods.json"
    );
}
