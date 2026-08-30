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
    /// Spawned by us, not yet handshaken — visible pre-handshake so fleet rows
    /// and startup-dialog watching exist from t=0 (v2 lifecycle keystone).
    Starting,
    /// IPC connected — usable.
    Connected,
    /// IPC down — temporary grace for resurrection / cancelled-task traces;
    /// evicted from the registry after TTL or when any other handshake succeeds.
    Disconnected,
}

/// Provenance of a spawned TD process. Kept across the Starting→Connected
/// transition; cleared when a pid is reused onto a different fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnRecord {
    /// When the process was spawned by us.
    pub started_at: DateTime<Utc>,
    /// Resolved TouchDesigner.exe path used for the spawn.
    pub exe_path: String,
    /// Project file passed at spawn, when known.
    pub expected_project: Option<String>,
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
    /// Responsive / frozen hint — filled by dialogs watcher when available (Windows + macOS), else None.
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
    /// Spawn provenance when this pid was launched via `spawn_td`.
    pub spawn: Option<SpawnRecord>,
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
    pub fn handshake(&mut self, pid: u32, process: ProcessAttrs, protocol_version: Option<String>) {
        match self.entries.get_mut(&pid) {
            Some(entry) => {
                if entry.bridge == BridgeStatus::Starting {
                    // Spawned process finally connected — keep provenance.
                    entry.process = process;
                    entry.protocol_version = protocol_version;
                    entry.bridge = BridgeStatus::Connected;
                    tracing::info!(pid, "pid handshake — spawned process connected");
                } else {
                    let same = entry.process.fingerprint.matches(&process.fingerprint);
                    if !same {
                        // Pid reuse onto a different process — clear state only.
                        entry.queue = TaskQueue::new();
                        entry.resurrection.clear();
                        entry.spawn = None;
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
                        spawn: None,
                    },
                );
                tracing::info!(pid, "pid handshake — new registration");
            }
        }
        self.evict_all_disconnected(Some(pid));
    }

    /// Register a freshly spawned process pre-handshake.
    ///
    /// Refuses to clobber a live Connected row (true pid collision); replacing
    /// a stale Starting/Disconnected row is fine — the old process is provably
    /// not usable. Returns whether the row was inserted.
    pub fn register_starting(&mut self, pid: u32, record: SpawnRecord) -> bool {
        match self.entries.get(&pid) {
            Some(entry) if entry.bridge == BridgeStatus::Connected => {
                tracing::warn!(pid, "refusing starting registration over connected row");
                false
            }
            _ => {
                self.entries.insert(
                    pid,
                    PidEntry {
                        pid,
                        bridge: BridgeStatus::Starting,
                        process: ProcessAttrs::default(),
                        queue: TaskQueue::new(),
                        resurrection: ResurrectionState::default(),
                        protocol_version: None,
                        spawn: Some(record),
                    },
                );
                true
            }
        }
    }

    /// Remove a Starting row (spawn waiter terminal-failure cleanup). Never
    /// touches Connected rows; returns whether anything was removed.
    pub fn remove_starting(&mut self, pid: u32) -> bool {
        match self.entries.get(&pid) {
            Some(entry) if entry.bridge == BridgeStatus::Starting => {
                self.entries.remove(&pid);
                true
            }
            _ => false,
        }
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

    fn spawn_record(exe: &str) -> SpawnRecord {
        SpawnRecord {
            started_at: Utc::now(),
            exe_path: exe.into(),
            expected_project: Some("C:/proj/x.toe".into()),
        }
    }

    #[test]
    fn starting_to_connected_preserves_spawn_record() {
        let mut r = PidRegistry::new();
        assert!(r.register_starting(7, spawn_record("C:/TD/TouchDesigner.exe")));
        assert_eq!(r.get(7).unwrap().bridge, BridgeStatus::Starting);
        r.handshake(7, attrs("proj"), Some("1".into()));
        let e = r.get(7).unwrap();
        assert_eq!(e.bridge, BridgeStatus::Connected);
        assert_eq!(
            e.spawn.as_ref().unwrap().exe_path,
            "C:/TD/TouchDesigner.exe"
        );
        assert_eq!(e.process.title.as_deref(), Some("proj"));
    }

    #[test]
    fn remove_starting_only_touches_starting_rows() {
        let mut r = PidRegistry::new();
        assert!(r.register_starting(7, spawn_record("e")));
        r.handshake(9, attrs("b"), Some("1".into()));
        assert!(!r.remove_starting(9), "connected row must survive");
        assert!(r.remove_starting(7));
        assert!(r.get(7).is_none());
        assert!(r.get(9).is_some());
    }

    #[test]
    fn register_starting_refuses_connected_pid() {
        let mut r = PidRegistry::new();
        r.handshake(5, attrs("live"), Some("1".into()));
        assert!(!r.register_starting(5, spawn_record("other")));
        assert_eq!(r.get(5).unwrap().bridge, BridgeStatus::Connected);
        // Stale Starting row is replaceable.
        assert!(r.register_starting(6, spawn_record("first")));
        assert!(r.register_starting(6, spawn_record("second")));
        assert_eq!(r.get(6).unwrap().spawn.as_ref().unwrap().exe_path, "second");
    }

    #[test]
    fn ghost_eviction_ignores_starting_rows() {
        let mut r = PidRegistry::new();
        r.handshake(1, attrs("a"), Some("1".into()));
        r.on_bridge_lost(1, Utc::now()); // disconnected ghost
        assert!(r.register_starting(2, spawn_record("e")));
        assert!(r.register_starting(3, spawn_record("e")));
        r.handshake(4, attrs("c"), Some("1".into())); // triggers eviction
        assert!(r.get(1).is_none(), "disconnected ghost evicted");
        assert_eq!(r.get(2).unwrap().bridge, BridgeStatus::Starting);
        assert_eq!(r.get(3).unwrap().bridge, BridgeStatus::Starting);
    }

    #[test]
    fn enqueue_rejects_starting_row() {
        let mut r = PidRegistry::new();
        r.register_starting(8, spawn_record("e"));
        let err = r.enqueue(8, "PythonEval", TaskMode::Shared).unwrap_err();
        assert!(matches!(err, EnqueueError::BridgeDisconnected { pid: 8 }));
    }

    #[test]
    fn pid_reuse_clears_spawn_record() {
        let mut r = PidRegistry::new();
        r.register_starting(11, spawn_record("old-exe"));
        r.handshake(11, attrs("proj"), Some("1".into()));
        // Same numeric pid, different fingerprint → fresh process.
        let mut fresh = attrs("other");
        fresh.fingerprint.title = Some("other".into());
        fresh.fingerprint.start_time = Some("t9".into());
        r.handshake(11, fresh, Some("1".into()));
        assert!(
            r.get(11).unwrap().spawn.is_none(),
            "reuse clears provenance"
        );
    }
}
