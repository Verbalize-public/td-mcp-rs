//! Cross-language / cross-crate numeric limits parity vs bridge/fixtures/limits.json.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests"
)]

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use tdmcp_config::BridgeSection;
use tdmcp_daemon::bridge::{
    BridgeTimeouts, HeartbeatConfig, CALL_TIMEOUT, HEARTBEAT_INTERVAL, IDLE_DEAD, PONG_TIMEOUT,
    SCRIPT_TIMEOUT,
};
use tdmcp_mcp::{
    BRIDGE_TIMEOUT, CHILDREN_ROSTER_LIMIT, EDITOR_PANES_LIMIT, EDITOR_SELECTION_LIMIT,
    INSPECT_PATHS_LIMIT,
};

#[derive(Debug, Deserialize)]
struct LimitsFixture {
    inspect_paths_limit: usize,
    children_roster_limit: usize,
    editor_selection_limit: usize,
    editor_panes_limit: usize,
    bridge_timeout_secs: u64,
    call_timeout_secs: u64,
    script_timeout_secs: u64,
    heartbeat_interval_secs: u64,
    pong_timeout_secs: u64,
    idle_dead_secs: u64,
}

fn load_fixture() -> LimitsFixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bridge/fixtures/limits.json");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).expect("parse limits fixture")
}

#[test]
fn rust_limits_match_shared_fixture() {
    let f = load_fixture();
    assert_eq!(INSPECT_PATHS_LIMIT, f.inspect_paths_limit);
    assert_eq!(CHILDREN_ROSTER_LIMIT, f.children_roster_limit);
    assert_eq!(EDITOR_SELECTION_LIMIT, f.editor_selection_limit);
    assert_eq!(EDITOR_PANES_LIMIT, f.editor_panes_limit);
    assert_eq!(BRIDGE_TIMEOUT, Duration::from_secs(f.bridge_timeout_secs));
    assert_eq!(CALL_TIMEOUT, Duration::from_secs(f.call_timeout_secs));
    assert_eq!(SCRIPT_TIMEOUT, Duration::from_secs(f.script_timeout_secs));
    assert_eq!(
        HEARTBEAT_INTERVAL,
        Duration::from_secs(f.heartbeat_interval_secs)
    );
    assert_eq!(PONG_TIMEOUT, Duration::from_secs(f.pong_timeout_secs));
    assert_eq!(IDLE_DEAD, Duration::from_secs(f.idle_dead_secs));

    let section = BridgeSection::default();
    assert_eq!(section.call_timeout_secs, f.call_timeout_secs);
    assert_eq!(section.script_timeout_secs, f.script_timeout_secs);
    assert_eq!(section.heartbeat_interval_secs, f.heartbeat_interval_secs);
    assert_eq!(section.pong_timeout_secs, f.pong_timeout_secs);
    assert_eq!(section.idle_dead_secs, f.idle_dead_secs);

    assert_eq!(
        HeartbeatConfig::production(),
        HeartbeatConfig::from(&section)
    );
    assert_eq!(BridgeTimeouts::production(), BridgeTimeouts::from(&section));
}
