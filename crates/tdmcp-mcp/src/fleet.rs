//! `fleet` tool — multi-process discovery (no sticky target).

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tdmcp_core::{BridgeStatus, DialogSnapshot, Pid, PidRegistry, PopupInfo, SpawnRecord};

/// Optional filters for `fleet`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FleetParams {
    /// Optional pid filter.
    #[serde(default)]
    pub pids: Option<Vec<Pid>>,
    /// Include sections (default: process + bridge). `tasks` omitted when empty;
    /// cancelled stack always when non-empty.
    #[serde(default)]
    pub include: Vec<FleetInclude>,
}

/// Sections that may be included in the fleet response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FleetInclude {
    /// Process attrs.
    Process,
    /// Bridge status.
    Bridge,
    /// Task queue snapshot.
    Tasks,
    /// Cancelled / resurrection traces.
    Cancelled,
    /// OS dialogs (best-effort; empty when backend disabled or unsupported).
    Popups,
}

/// One process row in the fleet response.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetProcess {
    /// OS pid.
    pub pid: Pid,
    /// Project identity (`project.name`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Responsive / frozen hint — filled by dialogs watcher when available, else None.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_status: Option<String>,
    /// Opened `.toe` path when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toe_path: Option<String>,
    /// Bridge status.
    pub bridge: BridgeStatus,
    /// Spawn provenance when we launched this process; absent for
    /// human-opened instances (v2 lifecycle ownership).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spawn: Option<SpawnRecord>,
    /// Open OS popups when include contains popups and a dialogs backend is
    /// installed; omitted otherwise/when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub popups: Vec<PopupInfo>,
    /// In-flight / pending tasks when requested; omitted when the snapshot is empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks: Option<Vec<tdmcp_core::TaskInfo>>,
    /// Jobs waiting in the bridge actor mpsc (not yet started). Present when
    /// `include` contains `tasks` and the transport reports a depth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipc_queue_depth: Option<usize>,
    /// Resurrected flag when non-default.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub resurrected: bool,
    /// Last disconnect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_disconnect_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Cancelled task stack (always when non-empty / resurrected).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cancelled_tasks: Vec<tdmcp_core::CancelledTask>,
    /// Owning daemon id when federated (local or remote).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_id: Option<String>,
    /// Owning hostname when federated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

/// Fleet tool response.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetResponse {
    /// Processes keyed by discovery order.
    pub processes: Vec<FleetProcess>,
}

/// Build a fleet summary from the registry.
///
/// `ipc_depths` maps pid → actor-inbox depth when the caller can observe it
/// (omit / empty when unknown). `popups` carries the daemon dialogs snapshot
/// map when installed (`None` = feature off / tests).
#[must_use]
pub fn fleet_summary(
    registry: &PidRegistry,
    params: &FleetParams,
    ipc_depths: &[(u32, usize)],
    popups: Option<&HashMap<u32, DialogSnapshot>>,
) -> FleetResponse {
    let want_tasks = params.include.contains(&FleetInclude::Tasks);
    let want_popups = params.include.contains(&FleetInclude::Popups);
    let filter = params.pids.as_ref();
    let mut processes = Vec::new();
    for pid in registry.pids() {
        if let Some(filter) = filter {
            if !filter.iter().any(|p| p.get() == pid) {
                continue;
            }
        }
        let Some(entry) = registry.get(pid) else {
            continue;
        };
        let ipc_queue_depth = want_tasks
            .then(|| ipc_depths.iter().find(|(p, _)| *p == pid).map(|(_, d)| *d))
            .flatten();
        let row_popups = match (want_popups, popups) {
            (true, Some(map)) => map.get(&pid).map(|s| s.popups.clone()).unwrap_or_default(),
            _ => Vec::new(),
        };
        processes.push(FleetProcess {
            pid: Pid::new(pid),
            title: entry.process.title.clone(),
            window_status: entry.process.window_status.clone(),
            toe_path: entry.process.toe_path.clone(),
            bridge: entry.bridge,
            spawn: entry.spawn.clone(),
            popups: row_popups,
            tasks: want_tasks
                .then(|| entry.queue.snapshot())
                .filter(|t| !t.is_empty()),
            ipc_queue_depth,
            resurrected: entry.resurrection.resurrected,
            last_disconnect_at: entry.resurrection.last_disconnect_at,
            cancelled_tasks: entry.resurrection.cancelled_tasks.clone(),
            daemon_id: None,
            hostname: None,
        });
    }
    FleetResponse { processes }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use tdmcp_core::{DialogSeverity, PopupKind, ProcessAttrs, ProcessFingerprint, TaskMode};

    fn connected_registry(pid: u32) -> PidRegistry {
        let mut reg = PidRegistry::new();
        reg.handshake(
            pid,
            ProcessAttrs {
                title: Some("proj".into()),
                fingerprint: ProcessFingerprint {
                    title: Some("proj".into()),
                    image: Some("TouchDesigner.exe".into()),
                    start_time: Some("t0".into()),
                },
                ..Default::default()
            },
            Some("1".into()),
        );
        reg
    }

    #[test]
    fn idle_tasks_include_omits_empty_tasks_key() {
        let reg = connected_registry(34);
        let params = FleetParams {
            include: vec![FleetInclude::Tasks, FleetInclude::Cancelled],
            ..Default::default()
        };
        let json =
            serde_json::to_value(fleet_summary(&reg, &params, &[], None)).expect("serialize");
        let proc = &json["processes"][0];
        assert_eq!(proc["pid"], 34);
        assert!(proc.get("tasks").is_none(), "idle queue must omit tasks");
        assert!(
            proc.get("cancelledTasks").is_none(),
            "empty cancelled stack must omit cancelledTasks"
        );
        assert!(proc.get("ipcQueueDepth").is_none());
    }

    #[test]
    fn non_empty_tasks_include_emits_tasks() {
        let mut reg = connected_registry(34);
        reg.enqueue(34, "PythonEval", TaskMode::Shared).unwrap();
        let params = FleetParams {
            include: vec![FleetInclude::Tasks],
            ..Default::default()
        };
        let json = serde_json::to_value(fleet_summary(&reg, &params, &[(34, 2)], None))
            .expect("serialize");
        let tasks = json["processes"][0]["tasks"]
            .as_array()
            .expect("tasks present when non-empty");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["name"], "PythonEval");
        assert_eq!(json["processes"][0]["ipcQueueDepth"], 2);
    }

    fn record(exe: &str) -> SpawnRecord {
        SpawnRecord {
            started_at: chrono::Utc::now(),
            exe_path: exe.into(),
            expected_project: Some("C:/p/x.toe".into()),
        }
    }

    #[test]
    fn spawned_starting_row_shows_provenance_and_status() {
        let mut reg = PidRegistry::new();
        reg.register_starting(50, record("C:/TD/TouchDesigner.exe"));
        let params = FleetParams::default();
        let json =
            serde_json::to_value(fleet_summary(&reg, &params, &[], None)).expect("serialize");
        let row = &json["processes"][0];
        assert_eq!(row["pid"], 50);
        assert_eq!(row["bridge"], "starting");
        assert_eq!(row["spawn"]["exePath"], "C:/TD/TouchDesigner.exe");
        assert!(row.get("owner").is_none() || row["owner"] == "spawned");
    }

    #[test]
    fn external_rows_omit_spawn_key() {
        let reg = connected_registry(9);
        let json = serde_json::to_value(fleet_summary(&reg, &FleetParams::default(), &[], None))
            .expect("s");
        let row = &json["processes"][0];
        assert_eq!(row["bridge"], "connected");
        assert!(
            row.get("spawn").is_none(),
            "human-opened rows must not claim provenance"
        );
    }

    #[test]
    fn spawned_connected_row_keeps_spawn_after_handshake() {
        let mut reg = PidRegistry::new();
        reg.register_starting(51, record("e"));
        reg.handshake(
            51,
            ProcessAttrs {
                title: Some("proj".into()),
                fingerprint: ProcessFingerprint {
                    title: Some("proj".into()),
                    image: Some("TouchDesigner.exe".into()),
                    start_time: Some("t1".into()),
                },
                ..Default::default()
            },
            Some("1".into()),
        );
        let json = serde_json::to_value(fleet_summary(&reg, &FleetParams::default(), &[], None))
            .expect("s");
        let row = &json["processes"][0];
        assert_eq!(row["bridge"], "connected");
        assert_eq!(row["spawn"]["expectedProject"], "C:/p/x.toe");
    }

    #[test]
    fn popups_include_emits_from_snapshots_and_omits_when_absent() {
        let reg = connected_registry(70);
        let params = FleetParams {
            include: vec![FleetInclude::Popups],
            ..Default::default()
        };
        // No dialogs map: field omitted entirely.
        let none = serde_json::to_value(fleet_summary(&reg, &params, &[], None)).unwrap();
        assert!(none["processes"][0].get("popups").is_none());

        let mut map = HashMap::new();
        map.insert(
            70,
            DialogSnapshot {
                popups: vec![PopupInfo {
                    id: "42".into(),
                    title: "Backwards Compatiblity Issue".into(),
                    class: Some("#32770".into()),
                    kind: PopupKind::MessageBox,
                    severity: DialogSeverity::Soft,
                    message: None,
                    buttons: Vec::new(),
                    is_main_chrome: false,
                }],
                window_status: Some(tdmcp_core::WindowStatus::BlockedByModalWindow),
            },
        );
        let with = serde_json::to_value(fleet_summary(&reg, &params, &[], Some(&map))).unwrap();
        let pops = &with["processes"][0]["popups"];
        assert_eq!(pops.as_array().unwrap().len(), 1);
        assert_eq!(pops[0]["severity"], "soft");
    }
}
