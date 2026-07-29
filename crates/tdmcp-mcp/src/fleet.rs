//! `fleet` tool — multi-process discovery (no sticky target).

use serde::{Deserialize, Serialize};
use tdmcp_core::{BridgeStatus, PidRegistry};

/// Optional filters for `fleet`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetParams {
    /// Optional pid filter.
    #[serde(default)]
    pub pids: Option<Vec<u32>>,
    /// Include sections (default: process + bridge; cancelled always when non-empty).
    #[serde(default)]
    pub include: Vec<FleetInclude>,
}

/// Sections that may be included in the fleet response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
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
    /// OS dialogs (best-effort; empty until dialogs land).
    Popups,
}

/// One process row in the fleet response.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetProcess {
    /// OS pid.
    pub pid: u32,
    /// Window title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Window status hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_status: Option<String>,
    /// Opened toe path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toe_path: Option<String>,
    /// Bridge status.
    pub bridge: BridgeStatus,
    /// In-flight / pending tasks when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks: Option<Vec<tdmcp_core::TaskInfo>>,
    /// Resurrected flag when non-default.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub resurrected: bool,
    /// Last disconnect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_disconnect_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Cancelled task stack (always when non-empty / resurrected).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cancelled_tasks: Vec<tdmcp_core::CancelledTask>,
}

/// Fleet tool response.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetResponse {
    /// Processes keyed by discovery order.
    pub processes: Vec<FleetProcess>,
}

/// Build a fleet summary from the registry.
#[must_use]
pub fn fleet_summary(registry: &PidRegistry, params: &FleetParams) -> FleetResponse {
    let want_tasks = params.include.contains(&FleetInclude::Tasks);
    let filter = params.pids.as_ref();
    let mut processes = Vec::new();
    for pid in registry.pids() {
        if let Some(filter) = filter {
            if !filter.contains(&pid) {
                continue;
            }
        }
        let Some(entry) = registry.get(pid) else {
            continue;
        };
        processes.push(FleetProcess {
            pid,
            title: entry.process.title.clone(),
            window_status: entry.process.window_status.clone(),
            toe_path: entry.process.toe_path.clone(),
            bridge: entry.bridge,
            tasks: want_tasks.then(|| entry.queue.snapshot()),
            resurrected: entry.resurrection.resurrected,
            last_disconnect_at: entry.resurrection.last_disconnect_at,
            cancelled_tasks: entry.resurrection.cancelled_tasks.clone(),
        });
    }
    FleetResponse { processes }
}
