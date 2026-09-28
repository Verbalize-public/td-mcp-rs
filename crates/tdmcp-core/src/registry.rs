//! Pid → bridge / process / queue / resurrection registry.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::bridge_identity::{
    validate_ready, BridgeCapabilityError, BridgeHeartbeatObservation, BridgeIdentityState,
    BridgeReadyEvent,
};
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
    /// Loaded identity/capability observations for the current connection.
    pub bridge_identity: BridgeIdentityState,
    /// Spawn provenance when this pid was launched via `spawn_td`.
    pub spawn: Option<SpawnRecord>,
}

/// Ground-truth map: OS pid → entry.
#[derive(Debug, Default)]
pub struct PidRegistry {
    entries: HashMap<u32, PidEntry>,
    next_connection_generation: u64,
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
    ) -> u64 {
        self.next_connection_generation = self.next_connection_generation.saturating_add(1).max(1);
        let connection_generation = self.next_connection_generation;
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
                        bridge_identity: BridgeIdentityState::default(),
                        spawn: None,
                    },
                );
                tracing::info!(pid, "pid handshake — new registration");
            }
        }
        if let Some(entry) = self.entries.get_mut(&pid) {
            entry
                .bridge_identity
                .begin_connection(connection_generation, Utc::now());
        }
        self.evict_all_disconnected(Some(pid));
        connection_generation
    }

    /// Record a valid ready event for the exact connected generation.
    pub fn observe_bridge_ready(
        &mut self,
        pid: u32,
        connection_generation: u64,
        ready: BridgeReadyEvent,
        observed_at: DateTime<Utc>,
    ) -> Result<(), BridgeCapabilityError> {
        validate_ready(&ready)?;
        let entry = self.connected_generation_mut(pid, connection_generation)?;
        if entry.protocol_version.as_deref() != Some(ready.protocol.version.as_str()) {
            return Err(BridgeCapabilityError::InvalidReady {
                reason: format!(
                    "ready protocol {} does not match handshake {:?}",
                    ready.protocol.version, entry.protocol_version
                ),
            });
        }
        entry.bridge_identity.ipc_observed_at = Some(observed_at);
        entry.bridge_identity.main_thread_pump =
            ready.main_thread_pump_age_ms.map(|age| (observed_at, age));
        entry.bridge_identity.ready = Some(ready);
        Ok(())
    }

    /// Record framed IPC traffic for the exact connected generation.
    pub fn observe_bridge_ipc(
        &mut self,
        pid: u32,
        connection_generation: u64,
        observed_at: DateTime<Utc>,
    ) -> Result<(), BridgeCapabilityError> {
        let entry = self.connected_generation_mut(pid, connection_generation)?;
        entry.bridge_identity.ipc_observed_at = Some(observed_at);
        Ok(())
    }

    /// Record heartbeat metadata without conflating worker IPC with the cached
    /// main-thread pump observation.
    pub fn observe_bridge_heartbeat(
        &mut self,
        pid: u32,
        connection_generation: u64,
        heartbeat: BridgeHeartbeatObservation,
        observed_at: DateTime<Utc>,
    ) -> Result<(), BridgeCapabilityError> {
        let entry = self.connected_generation_mut(pid, connection_generation)?;
        let Some(ready) = entry.bridge_identity.ready.as_ref() else {
            return Err(BridgeCapabilityError::UnknownCapabilities { pid });
        };
        if ready.load_generation != heartbeat.load_generation {
            return Err(BridgeCapabilityError::LoadGenerationMismatch { pid });
        }
        entry.bridge_identity.ipc_observed_at = Some(observed_at);
        if let Some(age) = heartbeat.main_thread_pump_age_ms {
            entry.bridge_identity.main_thread_pump = Some((observed_at, age));
        }
        Ok(())
    }

    /// Admission helper for future effectful dispatch. Ordinary legacy calls
    /// intentionally do not invoke this predicate.
    pub fn require_bridge_features<'a, I>(
        &self,
        pid: u32,
        connection_generation: u64,
        required: I,
    ) -> Result<(), BridgeCapabilityError>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let Some(entry) = self.entries.get(&pid) else {
            return Err(BridgeCapabilityError::UnknownPid { pid });
        };
        if entry.bridge != BridgeStatus::Connected {
            return Err(BridgeCapabilityError::NotConnected { pid });
        }
        if entry.bridge_identity.connection_generation != connection_generation {
            return Err(BridgeCapabilityError::StaleGeneration {
                pid,
                expected: entry.bridge_identity.connection_generation,
                observed: connection_generation,
            });
        }
        let Some(ready) = entry.bridge_identity.ready.as_ref() else {
            return Err(BridgeCapabilityError::UnknownCapabilities { pid });
        };
        let unsupported: Vec<String> = required
            .into_iter()
            .filter(|feature| {
                !ready
                    .supported_features
                    .iter()
                    .any(|supported| supported == feature)
            })
            .map(str::to_owned)
            .collect();
        if unsupported.is_empty() {
            Ok(())
        } else {
            Err(BridgeCapabilityError::UnsupportedFeatures {
                pid,
                features: unsupported,
            })
        }
    }

    fn connected_generation_mut(
        &mut self,
        pid: u32,
        connection_generation: u64,
    ) -> Result<&mut PidEntry, BridgeCapabilityError> {
        let Some(entry) = self.entries.get_mut(&pid) else {
            return Err(BridgeCapabilityError::UnknownPid { pid });
        };
        if entry.bridge != BridgeStatus::Connected {
            return Err(BridgeCapabilityError::NotConnected { pid });
        }
        if entry.bridge_identity.connection_generation != connection_generation {
            return Err(BridgeCapabilityError::StaleGeneration {
                pid,
                expected: entry.bridge_identity.connection_generation,
                observed: connection_generation,
            });
        }
        Ok(entry)
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
                        bridge_identity: BridgeIdentityState::default(),
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

    fn ready(load: &str, digest: &str) -> BridgeReadyEvent {
        BridgeReadyEvent {
            load_generation: load.into(),
            protocol: crate::BridgeProtocolIdentity {
                name: "tdmcp-bridge-ipc".into(),
                version: "1".into(),
            },
            runtime: crate::BridgeRuntimeIdentity {
                implementation: "CPython".into(),
                python_version: "3.11".into(),
                platform: "test".into(),
            },
            supported_features: vec!["inspect".into(), "ping".into()],
            loaded_code: crate::LoadedCodeIdentity {
                kind: "pythonCodeObjects".into(),
                algorithm: "sha256-python-marshal-v1".into(),
                digest: digest.into(),
                coverage: vec!["inspect".into()],
                missing: Vec::new(),
                limitations: vec!["bounded".into()],
            },
            source_snapshot: crate::BridgeSourceSnapshot {
                kind: "sourceSnapshot".into(),
                provenance: "onDiskAtReadyAfterFinalReload".into(),
                algorithm: "sha256-framed-files-v1".into(),
                digest: format!("source-{digest}"),
                files: vec!["inspect.py".into()],
                unavailable: Vec::new(),
                limitations: vec!["point in time".into()],
            },
            bundle_snapshot: crate::BridgeBundleSnapshot {
                kind: "bridgePackageSnapshot".into(),
                provenance: "daemonSelectedPackageOnDiskAtReadyAfterFinalReload".into(),
                algorithm: "sha256-framed-files-v1".into(),
                digest: format!("bundle-{digest}"),
                files: vec!["manifest.json".into(), "tdmcp_bridge/inspect.py".into()],
                unavailable: Vec::new(),
                limitations: vec!["point in time".into()],
            },
            main_thread_pump_age_ms: Some(25),
        }
    }

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

    #[test]
    fn old_bridge_without_ready_stays_capability_unknown() {
        let mut r = PidRegistry::new();
        let generation = r.handshake(12, attrs("legacy"), Some("1".into()));
        let entry = r.get(12).unwrap();
        assert_eq!(entry.bridge_identity.connection_generation, generation);
        assert!(entry.bridge_identity.ready.is_none());
        assert!(matches!(
            r.require_bridge_features(12, generation, ["inspect"]),
            Err(BridgeCapabilityError::UnknownCapabilities { pid: 12 })
        ));
    }

    #[test]
    fn ready_is_generation_bound_and_cleared_on_reconnect() {
        let mut r = PidRegistry::new();
        let first = r.handshake(13, attrs("proj"), Some("1".into()));
        r.observe_bridge_ready(13, first, ready("load-a", "aaa"), Utc::now())
            .unwrap();
        r.require_bridge_features(13, first, ["inspect"]).unwrap();
        assert!(matches!(
            r.require_bridge_features(13, first, ["evidence_read"]),
            Err(BridgeCapabilityError::UnsupportedFeatures { features, .. })
                if features == vec!["evidence_read"]
        ));

        let second = r.handshake(13, attrs("proj"), Some("1".into()));
        assert_ne!(first, second);
        assert!(r.get(13).unwrap().bridge_identity.ready.is_none());
        assert!(matches!(
            r.observe_bridge_ready(13, first, ready("stale", "bbb"), Utc::now()),
            Err(BridgeCapabilityError::StaleGeneration {
                expected,
                observed,
                ..
            }) if expected == second && observed == first
        ));
        assert!(matches!(
            r.require_bridge_features(13, second, ["inspect"]),
            Err(BridgeCapabilityError::UnknownCapabilities { .. })
        ));
    }

    #[test]
    fn same_protocol_version_can_have_different_loaded_identity() {
        let mut r = PidRegistry::new();
        let first = r.handshake(14, attrs("proj"), Some("1".into()));
        r.observe_bridge_ready(14, first, ready("load-a", "aaa"), Utc::now())
            .unwrap();
        let first_snapshot = r.get(14).unwrap().bridge_identity.snapshot(Utc::now());

        let second = r.handshake(14, attrs("proj"), Some("1".into()));
        r.observe_bridge_ready(14, second, ready("load-b", "bbb"), Utc::now())
            .unwrap();
        let second_snapshot = r.get(14).unwrap().bridge_identity.snapshot(Utc::now());
        assert_eq!(
            first_snapshot.protocol.as_ref().unwrap().version,
            second_snapshot.protocol.as_ref().unwrap().version
        );
        assert_ne!(
            first_snapshot.load_generation,
            second_snapshot.load_generation
        );
        assert_ne!(
            first_snapshot.loaded_code.unwrap().digest,
            second_snapshot.loaded_code.unwrap().digest
        );
    }

    #[test]
    fn heartbeat_cannot_invent_or_cross_load_pump_observation() {
        let mut r = PidRegistry::new();
        let generation = r.handshake(15, attrs("proj"), Some("1".into()));
        let mut initial = ready("load-a", "aaa");
        initial.main_thread_pump_age_ms = None;
        r.observe_bridge_ready(15, generation, initial, Utc::now())
            .unwrap();
        assert!(r
            .get(15)
            .unwrap()
            .bridge_identity
            .main_thread_pump
            .is_none());

        assert!(matches!(
            r.observe_bridge_heartbeat(
                15,
                generation,
                BridgeHeartbeatObservation {
                    load_generation: "other-load".into(),
                    main_thread_pump_age_ms: Some(1),
                },
                Utc::now(),
            ),
            Err(BridgeCapabilityError::LoadGenerationMismatch { pid: 15 })
        ));
        assert!(r
            .get(15)
            .unwrap()
            .bridge_identity
            .main_thread_pump
            .is_none());

        r.observe_bridge_heartbeat(
            15,
            generation,
            BridgeHeartbeatObservation {
                load_generation: "load-a".into(),
                main_thread_pump_age_ms: Some(5_000),
            },
            Utc::now(),
        )
        .unwrap();
        assert_eq!(
            r.get(15)
                .unwrap()
                .bridge_identity
                .main_thread_pump
                .unwrap()
                .1,
            5_000
        );
    }
}
