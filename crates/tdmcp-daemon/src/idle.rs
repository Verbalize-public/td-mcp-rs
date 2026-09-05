//! Daemon idle auto-exit: leave when no bridges and no MCP session leases.
//!
//! Gated by config `keep_alive` (see composition root). When armed, timeout
//! comes from env `TDMCP_IDLE_EXIT_SECS` (default 30; `0` disables).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tdmcp_mcp::McpSessionRegistry;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::bridge::BridgeSessions;

/// Default idle timeout when the env var is unset.
pub const DEFAULT_IDLE_EXIT_SECS: u64 = 30;

/// Grace after watcher start before idle may begin counting.
///
/// Covers the window after daemon (re)start when the stdio proxy has not yet
/// re-acquired a Streamable HTTP session lease. Without this, a short
/// `TDMCP_IDLE_EXIT_SECS` can kill a freshly-spawned daemon before heal lands.
pub const DEFAULT_IDLE_STARTUP_GRACE: Duration = Duration::from_secs(5);

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

/// Poll presence until continuous idle exceeds `timeout`, then toast and cancel.
///
/// Does not call `process::exit` — the composition root drains axum and ends
/// the process on the main thread.
pub async fn run_idle_watcher(
    bridges: BridgeSessions,
    mcp_sessions: Arc<McpSessionRegistry>,
    timeout: Duration,
    shutdown: CancellationToken,
    quit: Arc<AtomicBool>,
) {
    run_idle_watcher_with_grace(
        bridges,
        mcp_sessions,
        timeout,
        DEFAULT_IDLE_STARTUP_GRACE,
        shutdown,
        quit,
    )
    .await;
}

/// Like [`run_idle_watcher`] with an explicit startup grace (tests).
pub async fn run_idle_watcher_with_grace(
    bridges: BridgeSessions,
    mcp_sessions: Arc<McpSessionRegistry>,
    timeout: Duration,
    startup_grace: Duration,
    shutdown: CancellationToken,
    quit: Arc<AtomicBool>,
) {
    let poll = poll_interval(timeout);
    let started = Instant::now();
    let mut idle_since: Option<Instant> = None;

    loop {
        if shutdown.is_cancelled() {
            return;
        }

        let bridge_count = bridges.connected_count().await;
        let mcp_count = mcp_sessions.len();
        let busy = bridges.idle_exit_disabled() || bridge_count > 0 || mcp_count > 0;
        let in_startup_grace = started.elapsed() < startup_grace;

        if busy || in_startup_grace {
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
                quit.store(true, Ordering::SeqCst);
                shutdown.cancel();
                return;
            }
        }

        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(poll) => {}
        }
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

    #[tokio::test]
    async fn startup_grace_delays_idle_exit() {
        use std::sync::atomic::AtomicBool;
        use tdmcp_core::PidRegistry;
        use tdmcp_mcp::McpSessionRegistry;
        use tokio::sync::Mutex;

        let registry = Arc::new(Mutex::new(PidRegistry::new()));
        let bridges =
            BridgeSessions::new(registry).with_heartbeat(crate::HeartbeatConfig::disabled());
        let mcp_sessions = Arc::new(McpSessionRegistry::new());
        let shutdown = CancellationToken::new();
        let quit = Arc::new(AtomicBool::new(false));

        let watcher = tokio::spawn(run_idle_watcher_with_grace(
            bridges,
            mcp_sessions,
            Duration::from_millis(50),
            Duration::from_millis(200),
            shutdown.clone(),
            Arc::clone(&quit),
        ));

        // Still inside startup grace — must not have exited.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!quit.load(Ordering::SeqCst));
        assert!(!shutdown.is_cancelled());

        // After grace + idle timeout, should exit.
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(quit.load(Ordering::SeqCst));
        let _ = watcher.await;
    }

    #[tokio::test]
    async fn mcp_lease_blocks_idle_exit() {
        use std::sync::atomic::AtomicBool;
        use tdmcp_core::PidRegistry;
        use tdmcp_mcp::McpSessionRegistry;
        use tokio::sync::Mutex;

        let registry = Arc::new(Mutex::new(PidRegistry::new()));
        let bridges =
            BridgeSessions::new(registry).with_heartbeat(crate::HeartbeatConfig::disabled());
        let mcp_sessions = Arc::new(McpSessionRegistry::new());
        let _lease = mcp_sessions.acquire();
        let shutdown = CancellationToken::new();
        let quit = Arc::new(AtomicBool::new(false));

        let watcher = tokio::spawn(run_idle_watcher_with_grace(
            bridges,
            Arc::clone(&mcp_sessions),
            Duration::from_millis(40),
            Duration::from_millis(0),
            shutdown.clone(),
            Arc::clone(&quit),
        ));

        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            !quit.load(Ordering::SeqCst),
            "live MCP lease must block idle exit"
        );
        shutdown.cancel();
        let _ = watcher.await;
    }
}
