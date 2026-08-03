//! Handshake messages (first frames on a new connection).

use serde::{Deserialize, Serialize};

/// First message from TD → daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandshakeRequest {
    /// OS pid of the TD process.
    pub pid: u32,
    /// Protocol version the bridge client speaks.
    pub protocol_version: String,
    /// Optional project identity (`project.name`), not OS window title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional opened `.toe` path (`project.folder` + `project.name`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toe_path: Option<String>,
    /// Optional process image / exe path (fingerprint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Optional opaque OS process start-time (fingerprint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,
}

/// Optional budgets the daemon offers the bridge during handshake.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HandshakeOffer {
    /// Idle-dead budget (seconds) for the bridge serve loop.
    pub idle_dead_secs: Option<u64>,
    /// Upper bound (seconds) the bridge worker may wait for main-thread
    /// `process_pending` before failing the IPC call and unwedging.
    pub max_call_wait_secs: Option<u64>,
}

/// Daemon → TD: path to bridge package directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandshakeResponse {
    /// Absolute FS path to the bridge package directory.
    pub bridge_package_dir: String,
    /// Daemon protocol version.
    pub daemon_version: String,
    /// Minimum daemon version this package requires (echo / advisory).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_daemon: Option<String>,
    /// Idle-dead budget (seconds) the bridge should use in its serve loop.
    ///
    /// Optional for back-compat with older bridges; when set, the Python
    /// bridge passes it to `serve_queued(idle_dead_s=…)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_dead_secs: Option<u64>,
    /// Max seconds the bridge worker may block on main-thread dispatch.
    ///
    /// Optional for back-compat; Python defaults to 180s when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_call_wait_secs: Option<u64>,
}
