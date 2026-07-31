//! Daemon idle auto-exit: leave when no bridges and no MCP session leases.
//!
//! Env `TDMCP_IDLE_EXIT_SECS` (default 30; `0` disables).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tdmcp_mcp::McpSessionRegistry;
use tracing::info;

use crate::bridge::BridgeSessions;
use crate::ensure::daemon_lock_path;

/// Default idle timeout when the env var is unset.
pub const DEFAULT_IDLE_EXIT_SECS: u64 = 30;

/// Parse `TDMCP_IDLE_EXIT_SECS`. `None` means idle exit is disabled.
#[must_use]
pub fn idle_exit_timeout() -> Option<Duration> {
    idle_exit_timeout_from_env(std::env::var("TDMCP_IDLE_EXIT_SECS").ok().as_deref())
}

/// Parse an optional env value (tests).
#[must_use]
pub fn idle_exit_timeout_from_env(raw: Option<&str>) -> Option<Duration> {
    match raw {
        None => Some(Duration::from_secs(DEFAULT_IDLE_EXIT_SECS)),
        Some(s) => match s.trim().parse::<u64>() {
            Ok(0) => None,
            Ok(secs) => Some(Duration::from_secs(secs)),
            Err(_) => {
                tracing::warn!(
                    value = s,
                    "invalid TDMCP_IDLE_EXIT_SECS — using default {DEFAULT_IDLE_EXIT_SECS}s"
                );
                Some(Duration::from_secs(DEFAULT_IDLE_EXIT_SECS))
            }
        },
    }
}

/// Poll presence until continuous idle exceeds `timeout`, then toast and exit.
pub async fn run_idle_watcher(
    bridges: BridgeSessions,
    mcp_sessions: Arc<McpSessionRegistry>,
    data_dir: impl AsRef<Path>,
    timeout: Duration,
) {
    let data_dir = data_dir.as_ref().to_path_buf();
    let poll = poll_interval(timeout);
    let mut idle_since: Option<Instant> = None;

    loop {
        let bridge_count = bridges.connected_count().await;
        let mcp_count = mcp_sessions.len();
        let busy = bridge_count > 0 || mcp_count > 0;

        if busy {
            idle_since = None;
        } else {
            let since = idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= timeout {
                info!(
                    bridge_count,
                    mcp_count,
                    idle_secs = timeout.as_secs(),
                    "idle exit — no MCP or bridge clients"
                );
                notify_idle_exit(timeout);
                let lock = daemon_lock_path(&data_dir);
                let _ = std::fs::remove_file(&lock);
                tokio::time::sleep(Duration::from_millis(200)).await;
                #[allow(clippy::exit, reason = "idle exit process boundary")]
                std::process::exit(0);
            }
        }

        tokio::time::sleep(poll).await;
    }
}

fn poll_interval(timeout: Duration) -> Duration {
    let quarter = timeout / 4;
    if quarter.is_zero() {
        Duration::from_millis(50)
    } else {
        quarter.min(Duration::from_secs(1))
    }
}

fn notify_idle_exit(timeout: Duration) {
    let body = format!(
        "stopped — idle for {}s (no MCP or bridge clients)",
        timeout.as_secs().max(1)
    );
    #[cfg(feature = "gui")]
    {
        tdmcp_gui::toast("td-mcp-rs", &body);
    }
    #[cfg(not(feature = "gui"))]
    {
        let _ = body;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn default_when_unset() {
        assert_eq!(
            idle_exit_timeout_from_env(None),
            Some(Duration::from_secs(30))
        );
    }

    #[test]
    fn zero_disables() {
        assert_eq!(idle_exit_timeout_from_env(Some("0")), None);
        assert_eq!(idle_exit_timeout_from_env(Some(" 0 ")), None);
    }

    #[test]
    fn custom_secs() {
        assert_eq!(
            idle_exit_timeout_from_env(Some("2")),
            Some(Duration::from_secs(2))
        );
    }
}
