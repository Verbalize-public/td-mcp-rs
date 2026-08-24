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
    /// IPC down — temporary grace for resurrection / cancelled-task traces;
    /// evicted from the registry after TTL or when any other handshake succeeds.
    Disconnected,
}

/// Process attributes for discovery.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessAttrs {
    /// Project identity (`project.name` from handshake); not OS window title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Opened `.toe` path when known (`folder` + `name`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toe_path: Option<String>,
    /// Responsive / frozen hint — empty until P1 dialogs / hang probe.
    #[serde(skip_serializing_if = "Option::is_none")]
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
    ///
    /// Any successful handshake also evicts other disconnected fleet ghosts.
    pub fn handshake(
        &mut self,
        pid: u32,
        process: ProcessAttrs,
        protocol_version: Option<String>,
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
                    tracing::info!(pid, "pid handshake — reused onto a different process");
                } else {
                    let was_disconnected = entry.bridge == BridgeStatus::Disconnected;
                    entry.process = process;
                    entry.protocol_version = protocol_version;
                    entry.bridge = BridgeStatus::Connected;
                    if was_disconnected {
                        entry.resurrection.on_resurrect();
                        tracing::info!(pid, "pid handshake — resurrected");
                    } else {
                        tracing::info!(pid, "pid handshake — already connected");
                    }
                }
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
                tracing::info!(pid, "pid handshake — new registration");
            }
        }
        self.evict_all_disconnected(Some(pid));
    }

    /// Mark IPC lost: disconnect, cancel waits, stack traces.
    ///
    /// Not logged here — every caller already logs this fact at its own
    /// boundary (e.g. `bridge.rs`'s "bridge session ended" warn); rule §5.7.3
    /// (log once, at the boundary) applies.
    pub fn on_bridge_lost(&mut self, pid: u32, now: DateTime<Utc>) {
        let Some(entry) = self.entries.get_mut(&pid) else {
            return;
        };
        entry.bridge = BridgeStatus::Disconnected;
        let cancelled = entry.queue.cancel_all();
        entry.resurrection.on_bridge_lost(cancelled, now);
    }

    /// Clear pending/in-flight tasks without marking Disconnected or stacking
    /// resurrection (same-pid supersede: new actor already owns the connection).
    pub fn cancel_queue_keep_connected(&mut self, pid: u32) -> Vec<TaskInfo> {
        let Some(entry) = self.entries.get_mut(&pid) else {
            return Vec::new();
        };
        entry.queue.cancel_all()
    }

    /// Remove `pid` only when it is still disconnected. Returns whether removed.
    pub fn evict_if_disconnected(&mut self, pid: u32) -> bool {
        match self.entries.get(&pid) {
            Some(entry) if entry.bridge == BridgeStatus::Disconnected => {
                self.entries.remove(&pid);
                true
            }
            _ => false,
        }
    }

    /// Drop every disconnected entry except the optional pid (the connecting peer).
    pub fn evict_all_disconnected(&mut self, except: Option<u32>) {
        self.entries.retain(|pid, entry| {
            if Some(*pid) == except {
                return true;
            }
            entry.bridge != BridgeStatus::Disconnected
        });
    }

    /// Process exited — drop mapping unconditionally.
    pub fn on_process_exit(&mut self, pid: u32) {
        if self.entries.remove(&pid).is_some() {
            tracing::info!(pid, "pid registry entry dropped — process exited");
        }
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
        r.handshake(34, attrs("proj"), Some("1".into()));
        r.enqueue(34, "SharedA", TaskMode::Shared).unwrap();
        let err = r
            .enqueue(34, "ExclusiveB", TaskMode::Exclusive)
            .unwrap_err();
        assert!(matches!(err, EnqueueError::Queue(QueueError::Busy { .. })));
    }

    #[test]
    fn cancel_queue_keep_connected_clears_without_disconnect() {
        let mut r = PidRegistry::new();
        r.handshake(34, attrs("proj"), Some("1".into()));
        r.enqueue(34, "PythonEval", TaskMode::Shared).unwrap();
        r.start_next(34).unwrap();
        let cancelled = r.cancel_queue_keep_connected(34);
        assert_eq!(cancelled.len(), 1);
        let e = r.get(34).unwrap();
        assert_eq!(e.bridge, BridgeStatus::Connected);
        assert!(e.queue.is_empty());
        assert!(e.resurrection.cancelled_tasks.is_empty());
        r.enqueue(34, "ExclusiveB", TaskMode::Exclusive).unwrap();
    }

    #[test]
    fn pid_reuse_mismatch_clears_state_only() {
        let mut r = PidRegistry::new();
        r.handshake(34, attrs("old"), Some("1".into()));
        r.enqueue(34, "SharedA", TaskMode::Shared).unwrap();
        let now = Utc::now();
        r.on_bridge_lost(34, now);
        assert_eq!(r.get(34).unwrap().resurrection.cancelled_tasks.len(), 1);

        // Same numeric pid, different fingerprint → clear, no resurrected stack.
        let mut fresh = attrs("new");
        fresh.fingerprint.title = Some("new".into());
        fresh.fingerprint.start_time = Some("t1".into());
        r.handshake(34, fresh, Some("1".into()));

        let e = r.get(34).unwrap();
        assert_eq!(e.bridge, BridgeStatus::Connected);
        assert!(e.queue.is_empty());
        assert!(e.resurrection.cancelled_tasks.is_empty());
        assert!(!e.resurrection.resurrected);
    }

    #[test]
    fn resurrection_then_success_clears_stack() {
        let mut r = PidRegistry::new();
        r.handshake(34, attrs("proj"), Some("1".into()));
        r.enqueue(34, "PythonEval", TaskMode::Exclusive).unwrap();
        r.start_next(34).unwrap();
        let now = Utc::now();
        r.on_bridge_lost(34, now);
        r.handshake(34, attrs("proj"), Some("1".into()));
        assert!(r.get(34).unwrap().resurrection.resurrected);
        assert!(!r.get(34).unwrap().resurrection.cancelled_tasks.is_empty());

        r.enqueue(34, "PythonEval2", TaskMode::Shared).unwrap();
        r.start_next(34).unwrap();
        r.complete_task(34, TaskResult::Success).unwrap();
        assert!(!r.get(34).unwrap().resurrection.resurrected);
        assert!(r.get(34).unwrap().resurrection.cancelled_tasks.is_empty());
    }

    #[test]
    fn handshake_evicts_other_disconnected() {
        let mut r = PidRegistry::new();
        r.handshake(10, attrs("a"), Some("1".into()));
        r.handshake(20, attrs("b"), Some("1".into()));
        let now = Utc::now();
        r.on_bridge_lost(10, now);
        assert_eq!(r.get(10).unwrap().bridge, BridgeStatus::Disconnected);

        r.handshake(20, attrs("b"), Some("1".into()));
        assert!(r.get(10).is_none());
        assert_eq!(r.get(20).unwrap().bridge, BridgeStatus::Connected);
    }

    #[test]
    fn same_pid_handshake_resurrects_not_evicts() {
        let mut r = PidRegistry::new();
        r.handshake(10, attrs("a"), Some("1".into()));
        let now = Utc::now();
        r.on_bridge_lost(10, now);
        r.handshake(10, attrs("a"), Some("1".into()));
        let e = r.get(10).unwrap();
        assert_eq!(e.bridge, BridgeStatus::Connected);
        assert!(e.resurrection.resurrected);
    }

    #[test]
    fn evict_if_disconnected_noops_when_connected() {
        let mut r = PidRegistry::new();
        r.handshake(10, attrs("a"), Some("1".into()));
        assert!(!r.evict_if_disconnected(10));
        assert!(r.get(10).is_some());

        let now = Utc::now();
        r.on_bridge_lost(10, now);
        assert!(r.evict_if_disconnected(10));
        assert!(r.get(10).is_none());
    }
}


