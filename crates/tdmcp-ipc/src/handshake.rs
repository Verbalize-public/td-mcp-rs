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
    #[serde(default)]
    pub title: Option<String>,
    /// Optional opened `.toe` path (`project.folder` + `project.name`).
    #[serde(default)]
    pub toe_path: Option<String>,
    /// Optional process image / exe path (fingerprint).
    #[serde(default)]
    pub image: Option<String>,
    /// Optional opaque OS process start-time (fingerprint).
    #[serde(default)]
    pub start_time: Option<String>,
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
    #[serde(default)]
    pub min_daemon: Option<String>,
}
