//! Disconnect / cancelled-task stack / clear-on-first-success.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::task_queue::TaskInfo;

/// Why a task was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelReason {
    /// IPC link died.
    BridgeLost,
}

/// One cancelled-task trace entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelledTask {
    /// Task name.
    pub name: String,
    /// Exclusive flag.
    pub exclusive: bool,
    /// Reason.
    pub reason: CancelReason,
    /// When cancelled.
    pub cancelled_at: DateTime<Utc>,
}

/// Per-pid resurrection / loss attrs.
#[derive(Debug, Clone, Default)]
pub struct ResurrectionState {
    /// Last disconnect time.
    pub last_disconnect_at: Option<DateTime<Utc>>,
    /// Stacked cancelled tasks until first success.
    pub cancelled_tasks: Vec<CancelledTask>,
    /// True after same-pid re-handshake until first success clears.
    pub resurrected: bool,
}

impl ResurrectionState {
    /// Record an IPC loss and stack cancelled tasks.
    pub fn on_bridge_lost(
        &mut self,
        cancelled: impl IntoIterator<Item = TaskInfo>,
        now: DateTime<Utc>,
    ) {
        self.last_disconnect_at = Some(now);
        self.resurrected = false;
        for t in cancelled {
            self.cancelled_tasks.push(CancelledTask {
                name: t.name,
                exclusive: t.exclusive,
                reason: CancelReason::BridgeLost,
                cancelled_at: now,
            });
        }
    }

    /// Same pid re-handshaked — mark resurrected; keep stack.
    pub fn on_resurrect(&mut self) {
        self.resurrected = true;
    }

    /// Clear stack after a **successful** task. Failures keep the stack.
    pub fn on_task_outcome(&mut self, success: bool) {
        if success && (self.resurrected || !self.cancelled_tasks.is_empty()) {
            self.cancelled_tasks.clear();
            self.resurrected = false;
            self.last_disconnect_at = None;
        }
    }

    /// Wipe state (pid-reuse mismatch).
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use crate::task_queue::TaskInfo;

    #[test]
    fn first_task_fails_keeps_stack() {
        let mut s = ResurrectionState::default();
        let now = Utc::now();
        s.on_bridge_lost(
            [TaskInfo {
                name: "PythonEval".into(),
                exclusive: true,
            }],
            now,
        );
        s.on_resurrect();
        assert!(s.resurrected);
        assert_eq!(s.cancelled_tasks.len(), 1);

        s.on_task_outcome(false);
        assert!(s.resurrected);
        assert_eq!(s.cancelled_tasks.len(), 1);

        s.on_task_outcome(true);
        assert!(!s.resurrected);
        assert!(s.cancelled_tasks.is_empty());
        assert!(s.last_disconnect_at.is_none());
    }
}
