//! Federation identity and in-memory slave registry (zero I/O).

use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::registry::BridgeStatus;

/// Persistent daemon identity (UUID string).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DaemonId(String);

impl DaemonId {
    /// Wrap an owned id string.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrow the inner string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume into the inner string.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for DaemonId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Hash for DaemonId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl From<String> for DaemonId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for DaemonId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// Reachability of a registered slave as observed by the master.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SlaveReachability {
    /// Recent fleet-push received.
    Reachable,
    /// No push for ≥6s (processes shown greyed).
    Disconnected,
    /// No push for ≥10s.
    Unreachable,
}

/// Minimal remote process row for aggregated fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFleetProcess {
    /// OS pid on the remote daemon.
    pub pid: u32,
    /// Project identity when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Opened `.toe` path when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toe_path: Option<String>,
    /// Bridge status as reported by the slave (or forced on stale tick).
    pub bridge: BridgeStatus,
}

/// One registered slave on a master.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlaveEntry {
    /// Slave daemon id.
    pub daemon_id: DaemonId,
    /// Advertised hostname.
    pub hostname: String,
    /// Slave software version.
    pub version: String,
    /// Advertised base URL (`http://host:port`).
    pub base_url: String,
    /// Slave listen port.
    pub port: u16,
    /// Slave auth token for master→slave calls (slave `auth.psk`, or empty).
    pub auth_token: String,
    /// Last successful fleet-push time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fleet_push: Option<DateTime<Utc>>,
    /// Current reachability.
    pub reachability: SlaveReachability,
    /// Last pushed process list.
    #[serde(default)]
    pub fleet_processes: Vec<RemoteFleetProcess>,
}

/// Registration rejected because `daemon_id` is already bound to another URL.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("daemon_id {daemon_id} already registered at {existing_base_url} (got {attempted_base_url})")]
pub struct DaemonIdConflict {
    /// Conflicting id.
    pub daemon_id: DaemonId,
    /// URL already stored.
    pub existing_base_url: String,
    /// URL from the rejected register.
    pub attempted_base_url: String,
}

/// Outcome of [`SlaveRegistry::resolve_pid`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PidResolve {
    /// Pid exists on exactly one remote slave.
    Unique(DaemonId),
    /// Pid exists on multiple slaves.
    Ambiguous(Vec<(DaemonId, String)>),
    /// Treated as local (Fb: simplistic — not found on any slave).
    Local,
    /// Not found anywhere (Fb: unused when Local covers local-only).
    NotFound,
}

/// One process row in an aggregated master fleet.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AggregatedFleetProcess {
    /// OS pid.
    pub pid: u32,
    /// Project identity when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Opened `.toe` path when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toe_path: Option<String>,
    /// Bridge status.
    pub bridge: BridgeStatus,
    /// Owning daemon id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_id: Option<DaemonId>,
    /// Owning hostname.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

/// In-memory slave map. Callers wrap in `Arc<Mutex<_>>` at the daemon edge.
#[derive(Debug, Default, Clone)]
pub struct SlaveRegistry {
    slaves: HashMap<DaemonId, SlaveEntry>,
}

impl SlaveRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of registered slaves.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slaves.len()
    }

    /// True when no slaves are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slaves.is_empty()
    }

    /// Snapshot of all slaves (stable order by daemon_id).
    #[must_use]
    pub fn slaves(&self) -> Vec<SlaveEntry> {
        let mut out: Vec<SlaveEntry> = self.slaves.values().cloned().collect();
        out.sort_by(|a, b| a.daemon_id.as_str().cmp(b.daemon_id.as_str()));
        out
    }

    /// Look up a registered slave by id.
    #[must_use]
    pub fn get(&self, daemon_id: &DaemonId) -> Option<&SlaveEntry> {
        self.slaves.get(daemon_id)
    }

    /// Register or refresh a slave.
    ///
    /// Same `daemon_id` + same `base_url` → overwrite. Same id, different URL → conflict.
    pub fn register(&mut self, entry: SlaveEntry) -> Result<(), DaemonIdConflict> {
        if let Some(existing) = self.slaves.get(&entry.daemon_id) {
            if existing.base_url != entry.base_url {
                return Err(DaemonIdConflict {
                    daemon_id: entry.daemon_id,
                    existing_base_url: existing.base_url.clone(),
                    attempted_base_url: entry.base_url,
                });
            }
        }
        self.slaves.insert(entry.daemon_id.clone(), entry);
        Ok(())
    }

    /// Replace the process list for a slave and mark reachable.
    pub fn update_fleet(
        &mut self,
        daemon_id: &DaemonId,
        processes: Vec<RemoteFleetProcess>,
        now: DateTime<Utc>,
    ) -> bool {
        let Some(entry) = self.slaves.get_mut(daemon_id) else {
            return false;
        };
        entry.fleet_processes = processes;
        entry.last_fleet_push = Some(now);
        entry.reachability = SlaveReachability::Reachable;
        true
    }

    /// Apply stale thresholds: ≥6s → Disconnected, ≥10s → Unreachable.
    pub fn tick_stale(&mut self, now: DateTime<Utc>) {
        for entry in self.slaves.values_mut() {
            let Some(last) = entry.last_fleet_push else {
                // Never pushed — leave as registered (Reachable until first timeout window).
                // Treat "no push yet" using register time absence: if still Reachable with
                // no last_fleet_push, do not mark stale (register alone is enough for Fb).
                continue;
            };
            let age = now.signed_duration_since(last);
            let age_secs = age.num_seconds();
            if age_secs >= 10 {
                entry.reachability = SlaveReachability::Unreachable;
                for proc in &mut entry.fleet_processes {
                    proc.bridge = BridgeStatus::Disconnected;
                }
            } else if age_secs >= 6 {
                entry.reachability = SlaveReachability::Disconnected;
                for proc in &mut entry.fleet_processes {
                    proc.bridge = BridgeStatus::Disconnected;
                }
            }
        }
    }

    /// Resolve which daemon owns `pid` among slaves (Fb: Local when absent remotely).
    #[must_use]
    pub fn resolve_pid(&self, pid: u32) -> PidResolve {
        let mut hits: Vec<(DaemonId, String)> = Vec::new();
        for entry in self.slaves.values() {
            if entry
                .fleet_processes
                .iter()
                .any(|p| p.pid == pid && entry.reachability != SlaveReachability::Unreachable)
            {
                hits.push((entry.daemon_id.clone(), entry.hostname.clone()));
            }
        }
        match hits.len() {
            0 => PidResolve::Local,
            1 => PidResolve::Unique(hits.remove(0).0),
            _ => PidResolve::Ambiguous(hits),
        }
    }

    /// Merge local processes with slave fleets, tagging `daemon_id` / `hostname`.
    #[must_use]
    pub fn aggregate_fleet(
        &self,
        local_daemon_id: &DaemonId,
        local_hostname: &str,
        local_processes: Vec<AggregatedFleetProcess>,
    ) -> Vec<AggregatedFleetProcess> {
        let mut out = Vec::with_capacity(local_processes.len() + 16);
        for mut proc in local_processes {
            if proc.daemon_id.is_none() {
                proc.daemon_id = Some(local_daemon_id.clone());
            }
            if proc.hostname.is_none() {
                proc.hostname = Some(local_hostname.to_owned());
            }
            out.push(proc);
        }
        for slave in self.slaves() {
            for proc in slave.fleet_processes {
                out.push(AggregatedFleetProcess {
                    pid: proc.pid,
                    title: proc.title,
                    toe_path: proc.toe_path,
                    bridge: proc.bridge,
                    daemon_id: Some(slave.daemon_id.clone()),
                    hostname: Some(slave.hostname.clone()),
                });
            }
        }
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn entry(id: &str, base_url: &str) -> SlaveEntry {
        SlaveEntry {
            daemon_id: DaemonId::new(id),
            hostname: format!("host-{id}"),
            version: "0.1.2".into(),
            base_url: base_url.into(),
            port: 9860,
            auth_token: "tok".into(),
            last_fleet_push: None,
            reachability: SlaveReachability::Reachable,
            fleet_processes: vec![],
        }
    }

    #[test]
    fn register_overwrite_same_url() {
        let mut reg = SlaveRegistry::new();
        reg.register(entry("a", "http://127.0.0.1:1")).unwrap();
        let mut again = entry("a", "http://127.0.0.1:1");
        again.hostname = "renamed".into();
        reg.register(again).unwrap();
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.slaves()[0].hostname, "renamed");
    }

    #[test]
    fn register_conflict_different_url() {
        let mut reg = SlaveRegistry::new();
        reg.register(entry("a", "http://127.0.0.1:1")).unwrap();
        let err = reg
            .register(entry("a", "http://127.0.0.1:2"))
            .expect_err("conflict");
        assert_eq!(err.existing_base_url, "http://127.0.0.1:1");
        assert_eq!(err.attempted_base_url, "http://127.0.0.1:2");
    }

    #[test]
    fn update_fleet_and_resolve() {
        let mut reg = SlaveRegistry::new();
        reg.register(entry("a", "http://127.0.0.1:1")).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        assert!(reg.update_fleet(
            &DaemonId::new("a"),
            vec![RemoteFleetProcess {
                pid: 42,
                title: Some("proj".into()),
                toe_path: None,
                bridge: BridgeStatus::Connected,
            }],
            now,
        ));
        assert_eq!(
            reg.resolve_pid(42),
            PidResolve::Unique(DaemonId::new("a"))
        );
        assert_eq!(reg.resolve_pid(99), PidResolve::Local);
    }

    #[test]
    fn resolve_ambiguous() {
        let mut reg = SlaveRegistry::new();
        reg.register(entry("a", "http://127.0.0.1:1")).unwrap();
        reg.register(entry("b", "http://127.0.0.1:2")).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let proc = vec![RemoteFleetProcess {
            pid: 7,
            title: None,
            toe_path: None,
            bridge: BridgeStatus::Connected,
        }];
        reg.update_fleet(&DaemonId::new("a"), proc.clone(), now);
        reg.update_fleet(&DaemonId::new("b"), proc, now);
        match reg.resolve_pid(7) {
            PidResolve::Ambiguous(hits) => {
                assert_eq!(hits.len(), 2);
            }
            other => panic!("expected ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn tick_stale_thresholds() {
        let mut reg = SlaveRegistry::new();
        reg.register(entry("a", "http://127.0.0.1:1")).unwrap();
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        reg.update_fleet(
            &DaemonId::new("a"),
            vec![RemoteFleetProcess {
                pid: 1,
                title: None,
                toe_path: None,
                bridge: BridgeStatus::Connected,
            }],
            t0,
        );

        let t6 = t0 + chrono::Duration::seconds(6);
        reg.tick_stale(t6);
        assert_eq!(
            reg.slaves()[0].reachability,
            SlaveReachability::Disconnected
        );
        assert_eq!(reg.slaves()[0].fleet_processes[0].bridge, BridgeStatus::Disconnected);

        let t10 = t0 + chrono::Duration::seconds(10);
        reg.tick_stale(t10);
        assert_eq!(
            reg.slaves()[0].reachability,
            SlaveReachability::Unreachable
        );
    }

    #[test]
    fn aggregate_tags_local_and_remote() {
        let mut reg = SlaveRegistry::new();
        reg.register(entry("slave-1", "http://127.0.0.1:2")).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        reg.update_fleet(
            &DaemonId::new("slave-1"),
            vec![RemoteFleetProcess {
                pid: 9,
                title: Some("remote".into()),
                toe_path: None,
                bridge: BridgeStatus::Connected,
            }],
            now,
        );
        let local = vec![AggregatedFleetProcess {
            pid: 1,
            title: Some("local".into()),
            toe_path: None,
            bridge: BridgeStatus::Connected,
            daemon_id: None,
            hostname: None,
        }];
        let agg = reg.aggregate_fleet(&DaemonId::new("master"), "master-host", local);
        assert_eq!(agg.len(), 2);
        assert_eq!(agg[0].daemon_id.as_ref().unwrap().as_str(), "master");
        assert_eq!(agg[0].hostname.as_deref(), Some("master-host"));
        assert_eq!(agg[1].daemon_id.as_ref().unwrap().as_str(), "slave-1");
        assert_eq!(agg[1].pid, 9);
    }
}
