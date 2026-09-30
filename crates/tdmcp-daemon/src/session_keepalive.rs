//! The MCP session worker's keep-alive, derived from the budgets it must outlive.
//!
//! # Why this is derived and not a constant
//!
//! rmcp's session worker re-arms a keep-alive sleep on **every** loop iteration
//! and selects on it alongside incoming events
//! (`rmcp-3.0.1/src/transport/streamable_http_server/session/local.rs`:
//! `let keep_alive_timeout = tokio::time::sleep(keep_alive);` inside the
//! `tokio::select!`). A handler that blocks for a long time produces no
//! events, so nothing resets that timer and it fires *mid-request*: the
//! worker returns `IdleTimeout`, the session service terminates, and the
//! caller gets a transport error instead of the reply it is still waiting for.
//!
//! Measured live on this host, with duration as the only variable:
//!
//! | call | reply | payload |
//! |---|---|---|
//! | `spawn_td` `waitTimeoutMs=25000` | 25.1 s | `tdmcp.spawn.wait_timeout` with `pid`, `stillAlive`, `effect:"unknown"` |
//! | `spawn_td` `waitTimeoutMs=90000` | **no reply**, at exactly 60.0 s | `MCP stream ended without a response` |
//!
//! The 60 s was not any budget the caller asked for. It was this constant, and
//! the second call's `waitTimeoutMs` default of 60 000 sitting exactly on it is
//! why the *default* was the shape most likely to be met by it.
//!
//! So the value has one job: be strictly greater than the longest time a
//! handler can stay blocked. Every such budget is configured, so the ceiling
//! is derived from the configuration instead of pinned to a number that a
//! later config bump silently invalidates.

use std::time::Duration;

use tdmcp_config::ConfigFile;

/// Head-room added on top of the longest blocking budget.
///
/// The margin covers the work that happens *around* a call: the bridge
/// dispatch, the reply frame, and a client that is slow to read the SSE
/// stream. A keep-alive equal to the budget would race the reply it is meant
/// to protect, so the comparison is strict and the margin is what makes it
/// strict in practice.
pub const SESSION_KEEP_ALIVE_MARGIN: Duration = Duration::from_secs(30);

/// The longest a single tool handler can block, as configured.
///
/// `script_timeout_secs` is the largest daemon-side budget, but it is a
/// *bridge* budget: the time a bridged call may take once it has reached
/// TouchDesigner. `spawn_td` does not go through the bridge at all — it
/// blocks a Tokio task polling the process table for `waitTimeoutMs` — so the
/// spawn wait is the longer of the two and is the budget that actually has to
/// be outlived.
#[must_use]
pub fn longest_blocking_budget(cfg: &ConfigFile) -> Duration {
    let bridge_call = Duration::from_secs(cfg.bridge.call_timeout_secs.max(1));
    let bridge_script = Duration::from_secs(cfg.bridge.script_timeout_secs.max(1));
    // `spawn_td`'s default wait, from tdmcp-mcp's DEFAULT_SPAWN_WAIT_MS. A
    // caller may ask for more, but the daemon cannot know that number when
    // the transport is configured, so the ceiling is set where a longer wait
    // would have to be a configuration change — and it is documented that a
    // wait above the ceiling needs one.
    let spawn_wait = Duration::from_millis(tdmcp_mcp::DEFAULT_SPAWN_WAIT_MS);
    bridge_call.max(bridge_script).max(spawn_wait)
}

/// The MCP session worker's keep-alive for this configuration.
///
/// Strictly greater than [`longest_blocking_budget`]. There is no floor and
/// no ceiling beyond the margin: every budget is already at least the spawn
/// default, so a clamp would be a branch nothing can reach — a promise about
/// what the function does that no configuration could make true.
#[must_use]
pub fn session_keep_alive(cfg: &ConfigFile) -> Duration {
    longest_blocking_budget(cfg) + SESSION_KEEP_ALIVE_MARGIN
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "unit tests")]
mod tests {
    use super::*;
    use tdmcp_config::ConfigFile;

    fn cfg_with(call_secs: u64, script_secs: u64) -> ConfigFile {
        let mut cfg = ConfigFile::default();
        cfg.bridge.call_timeout_secs = call_secs;
        cfg.bridge.script_timeout_secs = script_secs;
        cfg
    }

    /// The measured shape, pinned as the property the fix exists for: no
    /// single call may be cut by the transport's own keep-alive.
    #[test]
    fn keep_alive_outlives_every_configured_budget() {
        for (call, script) in [(45_u64, 120_u64), (1, 1), (75, 300), (0, 0)] {
            let cfg = cfg_with(call, script);
            let keep = session_keep_alive(&cfg);
            let budget = longest_blocking_budget(&cfg);
            assert!(
                keep > budget,
                "keep-alive {keep:?} must exceed the longest blocking budget {budget:?} \
                 (call={call}s script={script}s), or a call in flight is cut off"
            );
        }
    }

    /// The regression this module was written for, in the shape it was seen:
    /// a 60s keep-alive with `spawn_td`'s 60s default wait on top of it.
    #[test]
    fn a_60s_keep_alive_would_cut_the_default_spawn_wait() {
        let cfg = cfg_with(45, 120);
        let keep = session_keep_alive(&cfg);
        let spawn_default = Duration::from_millis(tdmcp_mcp::DEFAULT_SPAWN_WAIT_MS);
        assert_ne!(
            keep,
            Duration::from_secs(60),
            "the shipped default wait equals the shipped 60s keep-alive, which is the \
             measured 'no reply at exactly 60.0s' failure"
        );
        assert!(keep > spawn_default);
    }

    /// A tiny configuration must not shrink the ceiling below the spawn wait
    /// the tool still uses, which is the floor in practice: 1s bridge budgets
    /// do not make `spawn_td` wait one second.
    #[test]
    fn a_tiny_configuration_still_outlives_the_spawn_default() {
        let cfg = cfg_with(1, 1);
        let keep = session_keep_alive(&cfg);
        assert!(keep > Duration::from_millis(tdmcp_mcp::DEFAULT_SPAWN_WAIT_MS));
    }

    /// The derivation must follow the configuration, so raising a budget in
    /// the config file actually raises the ceiling instead of being clipped
    /// by a constant nobody re-reads.
    #[test]
    fn raising_a_budget_raises_the_keep_alive() {
        let base = session_keep_alive(&cfg_with(45, 120));
        let raised = session_keep_alive(&cfg_with(45, 1_000));
        assert!(
            raised > base,
            "a 1000s script budget must not be outlived by a 120s-derived ceiling"
        );
    }

    /// The default configuration is the one the live measurements used, so it
    /// is worth naming the number rather than leaving it to arithmetic.
    #[test]
    fn the_default_configuration_gets_a_measured_ceiling() {
        let cfg = ConfigFile::default();
        assert_eq!(session_keep_alive(&cfg), Duration::from_secs(150));
    }
}
