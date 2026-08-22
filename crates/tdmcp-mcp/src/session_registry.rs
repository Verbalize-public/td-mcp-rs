//! Live Streamable HTTP MCP session registry (GUI + idle exit + session chill).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use uuid::Uuid;

/// Daemon scope key for local (non-proxied) bridged calls.
pub const DAEMON_SCOPE_LOCAL: &str = "local";

/// One live MCP client session visible to operators.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSessionInfo {
    /// Stable id for this lease (uuid v4).
    pub id: String,
    /// MCP `clientInfo.name` (or `"pending"` before initialize / annotate).
    pub client_name: String,
    /// MCP `clientInfo.version` (empty until known).
    pub client_version: String,
    /// Unix epoch milliseconds when the lease was acquired.
    pub connected_at: u64,
}

/// RAII guard for one `(session_id, daemon_scope, pid)` bridged-tool slot.
///
/// Dropping the guard (including on panic / early return) releases the slot.
#[derive(Debug)]
pub struct BridgeCallSlot<'a> {
    registry: &'a McpSessionRegistry,
    session_id: String,
    daemon_scope: String,
    pid: u32,
}

impl Drop for BridgeCallSlot<'_> {
    fn drop(&mut self) {
        self.registry
            .end_bridge_call(&self.session_id, &self.daemon_scope, self.pid);
    }
}

/// Shared registry of live MCP session leases.
#[derive(Debug, Default)]
pub struct McpSessionRegistry {
    inner: Mutex<Vec<McpSessionInfo>>,
    /// In-flight bridged tools keyed by `(session_id, daemon_scope, pid)`.
    bridge_inflight: Mutex<HashSet<(String, String, u32)>>,
}

impl McpSessionRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a new lease row; returns its id.
    #[must_use]
    pub fn acquire(self: &Arc<Self>) -> String {
        let id = Uuid::new_v4().to_string();
        let connected_at = now_ms();
        if let Ok(mut guard) = self.inner.lock() {
            guard.push(McpSessionInfo {
                id: id.clone(),
                client_name: "pending".into(),
                client_version: String::new(),
                connected_at,
            });
        }
        id
    }

    /// Remove a lease by id (no-op if already gone) and drop its chill slots.
    pub fn release(&self, id: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.retain(|s| s.id != id);
        }
        if let Ok(mut inflight) = self.bridge_inflight.lock() {
            inflight.retain(|(sid, _, _)| sid != id);
        }
    }

    /// Try to reserve one bridged-tool slot for `(session_id, daemon_scope, pid)`.
    ///
    /// Returns [`None`] when that triple already has an in-flight call (N=1).
    pub fn try_begin_bridge_call<'a>(
        &'a self,
        session_id: &str,
        daemon_scope: &str,
        pid: u32,
    ) -> Option<BridgeCallSlot<'a>> {
        let key = (session_id.to_owned(), daemon_scope.to_owned(), pid);
        let inserted = self
            .bridge_inflight
            .lock()
            .ok()
            .is_some_and(|mut g| g.insert(key));
        if !inserted {
            return None;
        }
        Some(BridgeCallSlot {
            registry: self,
            session_id: session_id.to_owned(),
            daemon_scope: daemon_scope.to_owned(),
            pid,
        })
    }

    /// Release a bridged-tool slot (also invoked by [`BridgeCallSlot`] Drop).
    pub fn end_bridge_call(&self, session_id: &str, daemon_scope: &str, pid: u32) {
        if let Ok(mut g) = self.bridge_inflight.lock() {
            g.remove(&(session_id.to_owned(), daemon_scope.to_owned(), pid));
        }
    }

    /// Whether `(session_id, daemon_scope, pid)` currently holds a bridged-tool slot (tests).
    #[must_use]
    pub fn has_bridge_call(&self, session_id: &str, daemon_scope: &str, pid: u32) -> bool {
        self.bridge_inflight
            .lock()
            .ok()
            .is_some_and(|g| g.contains(&(session_id.to_owned(), daemon_scope.to_owned(), pid)))
    }

    /// Set client identity after MCP `initialize` (or annotate).
    pub fn set_client_info(&self, id: &str, name: impl Into<String>, version: impl Into<String>) {
        let name = name.into();
        let version = version.into();
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(row) = guard.iter_mut().find(|s| s.id == id) {
                row.client_name = name;
                row.client_version = version;
            }
        }
    }

    /// Annotate by id. Returns `true` if a row was updated.
    pub fn annotate(&self, id: &str, name: impl Into<String>, version: impl Into<String>) -> bool {
        let name = name.into();
        let version = version.into();
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(row) = guard.iter_mut().find(|s| s.id == id) {
                row.client_name = name;
                row.client_version = version;
                return true;
            }
        }
        false
    }

    /// Annotate the newest session whose `client_name` matches `match_name`.
    pub fn annotate_latest_matching(
        &self,
        match_name: &str,
        name: impl Into<String>,
        version: impl Into<String>,
    ) -> Option<String> {
        let name = name.into();
        let version = version.into();
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(row) = guard.iter_mut().rev().find(|s| s.client_name == match_name) {
                row.client_name = name;
                row.client_version = version;
                return Some(row.id.clone());
            }
        }
        None
    }

    /// Snapshot of all live sessions (oldest first).
    #[must_use]
    pub fn list(&self) -> Vec<McpSessionInfo> {
        self.inner.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Live lease count (idle exit / status chips).
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Whether there are no live leases.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn acquire_set_release_counts() {
        let reg = Arc::new(McpSessionRegistry::new());
        assert_eq!(reg.len(), 0);
        let id = reg.acquire();
        assert_eq!(reg.len(), 1);
        reg.set_client_info(&id, "Cursor", "1.0.0");
        let list = reg.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].client_name, "Cursor");
        assert_eq!(list[0].client_version, "1.0.0");
        reg.release(&id);
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn annotate_latest_matching() {
        let reg = Arc::new(McpSessionRegistry::new());
        let a = reg.acquire();
        let b = reg.acquire();
        reg.set_client_info(&a, "tdmcp-stdio-proxy", "0.1.0");
        reg.set_client_info(&b, "tdmcp-stdio-proxy", "0.1.0");
        let updated = reg
            .annotate_latest_matching("tdmcp-stdio-proxy", "Cursor", "0.42")
            .expect("match");
        assert_eq!(updated, b);
        let list = reg.list();
        assert_eq!(list[0].client_name, "tdmcp-stdio-proxy");
        assert_eq!(list[1].client_name, "Cursor");
        assert_eq!(list[1].client_version, "0.42");
    }

    #[test]
    fn bridge_call_slot_n1_per_session_scope_pid() {
        let reg = McpSessionRegistry::new();
        let a = reg
            .try_begin_bridge_call("sess-1", DAEMON_SCOPE_LOCAL, 10)
            .expect("first acquire");
        assert!(reg.has_bridge_call("sess-1", DAEMON_SCOPE_LOCAL, 10));
        assert!(reg
            .try_begin_bridge_call("sess-1", DAEMON_SCOPE_LOCAL, 10)
            .is_none());
        // Same session+pid, different daemon scope is allowed.
        let remote = reg
            .try_begin_bridge_call("sess-1", "slave-a", 10)
            .expect("remote scope");
        // Different pid on same session is allowed.
        let b = reg
            .try_begin_bridge_call("sess-1", DAEMON_SCOPE_LOCAL, 11)
            .expect("other pid");
        // Different session on same pid is allowed (pid exclusive is separate).
        let c = reg
            .try_begin_bridge_call("sess-2", DAEMON_SCOPE_LOCAL, 10)
            .expect("other session");
        drop(a);
        assert!(!reg.has_bridge_call("sess-1", DAEMON_SCOPE_LOCAL, 10));
        let a2 = reg
            .try_begin_bridge_call("sess-1", DAEMON_SCOPE_LOCAL, 10)
            .expect("re-acquire after drop");
        assert!(reg.has_bridge_call("sess-1", DAEMON_SCOPE_LOCAL, 10));
        drop(a2);
        drop(remote);
        drop(b);
        drop(c);
    }

    #[test]
    fn release_session_clears_bridge_slots() {
        let reg = Arc::new(McpSessionRegistry::new());
        let id = reg.acquire();
        let slot = reg
            .try_begin_bridge_call(&id, DAEMON_SCOPE_LOCAL, 42)
            .expect("slot");
        assert!(reg.has_bridge_call(&id, DAEMON_SCOPE_LOCAL, 42));
        // Disconnect clears slots even while a guard is still alive.
        reg.release(&id);
        assert!(!reg.has_bridge_call(&id, DAEMON_SCOPE_LOCAL, 42));
        drop(slot); // must not panic after session release
    }
}
