//! Pid → bridge / process / queue / resurrection registry.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::fingerprint::ProcessFingerprint;
use crate::resurrection::ResurrectionState;
use crate::task_queue::{QueueError, TaskInfo, TaskMode, TaskQueue, TaskResult};

/// Bridge liveness for a pid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BridgeStatus {
    /// IPC connected — usable.
    Connected,
    /// Process known but IPC down — discovery only.
    Disconnected,
}

/// Process attributes for discovery.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessAttrs {
    /// Window title.
    pub title: Option<String>,
    /// Opened `.toe` path when known.
    pub toe_path: Option<String>,
    /// Responsive / frozen hint.
    pub window_status: Option<String>,
    /// Fingerprint for pid-reuse checks.
    pub fingerprint: ProcessFingerprint,
}

/// One registered TD process.
#[derive(Debug)]
pub struct PidEntry {
    /// OS pid.
    pub pid: u32,
    /// Bridge status.
    pub bridge: BridgeStatus,
    /// Process attrs.
    pub process: ProcessAttrs,
    /// Per-pid queue.
    pub queue: TaskQueue,
    /// Loss / resurrection attrs.
    pub resurrection: ResurrectionState,
    /// Protocol version from last handshake.
    pub protocol_version: Option<String>,
}

/// Ground-truth map: OS pid → entry.
#[derive(Debug, Default)]
pub struct PidRegistry {
    entries: HashMap<u32, PidEntry>,
}

impl PidRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// List all pids.
    #[must_use]
    pub fn pids(&self) -> Vec<u32> {
        let mut pids: Vec<u32> = self.entries.keys().copied().collect();
        pids.sort_unstable();
        pids
    }

    /// Get entry.
    #[must_use]
    pub fn get(&self, pid: u32) -> Option<&PidEntry> {
        self.entries.get(&pid)
    }

    /// Get mutable entry.
    pub fn get_mut(&mut self, pid: u32) -> Option<&mut PidEntry> {
        self.entries.get_mut(&pid)
    }

    /// Handshake from a connecting TD. Handles resurrection vs pid-reuse.
    pub fn handshake(
        &mut self,
        pid: u32,
        process: ProcessAttrs,
        protocol_version: Option<String>,
        now: DateTime<Utc>,
    ) {
        match self.entries.get_mut(&pid) {
            Some(entry) => {
                let same = entry.process.fingerprint.matches(&process.fingerprint);
                if !same {
                    // Pid reuse onto a different process — clear state only.
                    entry.queue = TaskQueue::new();
                    entry.resurrection.clear();
                    entry.process = process;
                    entry.protocol_version = protocol_version;
                    entry.bridge = BridgeStatus::Connected;
                    return;
                }
                let was_disconnected = entry.bridge == BridgeStatus::Disconnected;
                entry.process = process;
                entry.protocol_version = protocol_version;
                entry.bridge = BridgeStatus::Connected;
                if was_disconnected {
                    entry.resurrection.on_resurrect();
                }
                let _ = now;
            }
            None => {
                self.entries.insert(
                    pid,
                    PidEntry {
                        pid,
                        bridge: BridgeStatus::Connected,
                        process,
                        queue: TaskQueue::new(),
                        resurrection: ResurrectionState::default(),
                        protocol_version,
                    },
                );
            }
        }
    }

    /// Mark IPC lost: disconnect, cancel waits, stack traces.
    pub fn on_bridge_lost(&mut self, pid: u32, now: DateTime<Utc>) {
        let Some(entry) = self.entries.get_mut(&pid) else {
            return;
        };
        entry.bridge = BridgeStatus::Disconnected;
        let cancelled = entry.queue.cancel_all();
        entry.resurrection.on_bridge_lost(cancelled, now);
    }

    /// Process exited — drop mapping.
    pub fn on_process_exit(&mut self, pid: u32) {
        self.entries.remove(&pid);
    }

    /// Enqueue a task for a connected pid.
    pub fn enqueue(
        &mut self,
        pid: u32,
        name: impl Into<String>,
        mode: TaskMode,
    ) -> Result<(), EnqueueError> {
        let entry = self
            .entries
            .get_mut(&pid)
            .ok_or(EnqueueError::UnknownPid { pid })?;
        if entry.bridge != BridgeStatus::Connected {
            return Err(EnqueueError::BridgeDisconnected { pid });
        }
        entry
            .queue
            .try_enqueue(name, mode)
            .map_err(EnqueueError::Queue)
    }

    /// Record task completion; clear resurrection stack on success.
    pub fn complete_task(
        &mut self,
        pid: u32,
        result: TaskResult,
    ) -> Result<TaskInfo, EnqueueError> {
        let entry = self
            .entries
            .get_mut(&pid)
            .ok_or(EnqueueError::UnknownPid { pid })?;
        let info = entry
            .queue
            .complete_in_flight(result)
            .map_err(EnqueueError::Queue)?;
        entry
            .resurrection
            .on_task_outcome(matches!(result, TaskResult::Success));
        Ok(info)
    }

    /// Start next pending task.
    pub fn start_next(&mut self, pid: u32) -> Result<Option<TaskInfo>, EnqueueError> {
        let entry = self
            .entries
            .get_mut(&pid)
            .ok_or(EnqueueError::UnknownPid { pid })?;
        Ok(entry.queue.start_next().cloned())
    }
}

/// Errors from registry operations.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EnqueueError {
    /// Pid not in registry.
    #[error("unknown_pid: {pid}")]
    UnknownPid {
        /// Pid.
        pid: u32,
    },
    /// Bridge not connected.
    #[error("bridge disconnected for pid {pid}")]
    BridgeDisconnected {
        /// Pid.
        pid: u32,
    },
    /// Queue rejected the enqueue.
    #[error(transparent)]
    Queue(#[from] QueueError),
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    fn attrs(title: &str) -> ProcessAttrs {
        ProcessAttrs {
            title: Some(title.into()),
            fingerprint: ProcessFingerprint {
                title: Some(title.into()),
                image: Some("TouchDesigner.exe".into()),
                start_time: Some("t0".into()),
            },
            ..Default::default()
        }
    }

    #[test]
    fn exclusive_fails_while_busy_via_registry() {
        let mut r = PidRegistry::new();
        let now = Utc::now();
        r.handshake(34, attrs("proj"), Some("1".into()), now);
        r.enqueue(34, "SharedA", TaskMode::Shared).unwrap();
        let err = r
            .enqueue(34, "ExclusiveB", TaskMode::Exclusive)
            .unwrap_err();
        assert!(matches!(err, EnqueueError::Queue(QueueError::Busy { .. })));
    }

    #[test]
    fn pid_reuse_mismatch_clears_state_only() {
        let mut r = PidRegistry::new();
        let now = Utc::now();
        r.handshake(34, attrs("old"), Some("1".into()), now);
        r.enqueue(34, "SharedA", TaskMode::Shared).unwrap();
        r.on_bridge_lost(34, now);
        assert_eq!(r.get(34).unwrap().resurrection.cancelled_tasks.len(), 1);

        // Same numeric pid, different fingerprint → clear, no resurrected stack.
        let mut fresh = attrs("new");
        fresh.fingerprint.title = Some("new".into());
        fresh.fingerprint.start_time = Some("t1".into());
        r.handshake(34, fresh, Some("1".into()), now);

        let e = r.get(34).unwrap();
        assert_eq!(e.bridge, BridgeStatus::Connected);
        assert!(e.queue.is_empty());
        assert!(e.resurrection.cancelled_tasks.is_empty());
        assert!(!e.resurrection.resurrected);
    }

    #[test]
    fn resurrection_then_success_clears_stack() {
        let mut r = PidRegistry::new();
        let now = Utc::now();
        r.handshake(34, attrs("proj"), Some("1".into()), now);
        r.enqueue(34, "PythonEval", TaskMode::Exclusive).unwrap();
        r.start_next(34).unwrap();
        r.on_bridge_lost(34, now);
        r.handshake(34, attrs("proj"), Some("1".into()), now);
        assert!(r.get(34).unwrap().resurrection.resurrected);
        assert!(!r.get(34).unwrap().resurrection.cancelled_tasks.is_empty());

        r.enqueue(34, "PythonEval2", TaskMode::Shared).unwrap();
        r.start_next(34).unwrap();
        r.complete_task(34, TaskResult::Success).unwrap();
        assert!(!r.get(34).unwrap().resurrection.resurrected);
        assert!(r.get(34).unwrap().resurrection.cancelled_tasks.is_empty());
    }
}
