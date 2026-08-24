//! Central log record model shared by the file sink, ring, and (later)
//! bridge/proxy uplink. One JSON line per record; the schema is the lossless
//! inverse of `docs/OBSERVABILITY.md` §5.0.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Severity, serialized lowercase (`trace|debug|info|warn|error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Trace.
    Trace,
    /// Debug.
    Debug,
    /// Info.
    Info,
    /// Warn.
    Warn,
    /// Error.
    Error,
}

impl From<&tracing::core::Level> for Level {
    fn from(level: &tracing::core::Level) -> Self {
        match *level {
            tracing::core::Level::TRACE => Level::Trace,
            tracing::core::Level::DEBUG => Level::Debug,
            tracing::core::Level::INFO => Level::Info,
            tracing::core::Level::WARN => Level::Warn,
            tracing::core::Level::ERROR => Level::Error,
        }
    }
}

/// Record origin. Bridge/proxy records arrive pre-stamped by their producer;
/// in-process records are inferred from the tracing target prefix
/// (`tdmcp_ipc` → Ipc, `tdmcp_mcp` → Mcp, `tdmcp_gui` → Gui, else Daemon).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Src {
    /// Daemon process internals.
    Daemon,
    /// Bridge IPC session layer.
    Ipc,
    /// MCP tool layer.
    Mcp,
    /// Stdio proxy process (pre-stamped).
    Proxy,
    /// Tray GUI process (pre-stamped).
    Gui,
    /// TD-side Python bridge (pre-stamped).
    Bridge,
}

impl Src {
    /// Inference rule documented in-module: target-prefix based; everything
    /// unrecognized in-process is `Daemon`.
    pub fn infer_from_target(target: &str) -> Src {
        if target.starts_with("tdmcp_ipc") {
            Src::Ipc
        } else if target.starts_with("tdmcp_mcp") {
            Src::Mcp
        } else if target.starts_with("tdmcp_gui") {
            Src::Gui
        } else {
            Src::Daemon
        }
    }
}

/// One structured log record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    /// Monotonic sequence assigned by the ring on arrival (never trusted from
    /// a payload).
    pub seq: u64,
    /// RFC3339 UTC with milliseconds (arrival time).
    pub ts: String,
    /// Severity.
    pub level: Level,
    /// Origin.
    pub src: Src,
    /// Emitting process id.
    pub pid: u32,
    /// Tracing target / producer module path.
    pub target: String,
    /// Message body.
    pub msg: String,
    /// Diagnostics catalog code when one applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Structured key/values (sorted for stable output).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kvs: BTreeMap<String, String>,
}

/// Serialize one record as a JSON line (no trailing newline in the return
/// value; callers append `\n`).
pub fn to_line(r: &Record) -> String {
    serde_json::to_string(r).unwrap_or_else(|_| {
        // Serialization of this plain-data struct cannot fail in practice;
        // fall back to a minimal valid line rather than panicking (lib path).
        format!(
            "{{\"seq\":{},\"ts\":{:?},\"level\":\"info\",\"src\":\"daemon\",\"pid\":{},\"target\":\"logrecord\",\"msg\":\"serialize fallback\"}}",
            r.seq, r.ts, r.pid
        )
    })
}

/// Tolerant inverse of [`to_line`]: unknown fields ignored, missing optional
/// fields defaulted. Returns None on non-JSON lines.
pub fn from_line(line: &str) -> Option<Record> {
    serde_json::from_str(line.trim()).ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    fn sample() -> Record {
        Record {
            seq: 41,
            ts: "2026-01-01T12:00:00.123Z".to_owned(),
            level: Level::Warn,
            src: Src::Bridge,
            pid: 12_345,
            target: "bridge::tox_callbacks".to_owned(),
            msg: "heartbeat pong timeout".to_owned(),
            code: Some("tdmcp.bridge.pong_timeout".to_owned()),
            kvs: BTreeMap::from([("ms".to_owned(), "42".to_owned())]),
        }
    }

    #[test]
    fn line_round_trip_is_lossless() {
        let r = sample();
        let parsed = from_line(&to_line(&r)).expect("parse own line");
        assert_eq!(parsed, r);
    }

    #[test]
    fn line_omits_empty_optionals() {
        let mut r = sample();
        r.code = None;
        r.kvs.clear();
        let line = to_line(&r);
        assert!(!line.contains("code"));
        assert!(!line.contains("\"kvs\""));
        assert_eq!(from_line(&line).expect("parse"), r);
    }

    #[test]
    fn from_line_tolerates_unknown_fields() {
        let line = concat!(
            "{\"seq\":1,\"ts\":\"t\",\"level\":\"error\",\"src\":\"proxy\",\"pid\":7,",
            "\"target\":\"x\",\"msg\":\"m\",\"surprise\":{\"a\":1}}"
        );
        let r = from_line(line).expect("tolerant parse");
        assert_eq!(r.level, Level::Error);
        assert_eq!(r.src, Src::Proxy);
        assert_eq!(r.kvs, BTreeMap::new());
    }

    #[test]
    fn from_line_rejects_non_json() {
        assert!(from_line("not json").is_none());
        assert!(from_line("").is_none());
    }

    #[test]
    fn src_inference_by_target_prefix() {
        assert_eq!(Src::infer_from_target("tdmcp_ipc::session"), Src::Ipc);
        assert_eq!(Src::infer_from_target("tdmcp_mcp::tools"), Src::Mcp);
        assert_eq!(Src::infer_from_target("tdmcp_gui"), Src::Gui);
        assert_eq!(Src::infer_from_target("hyper::server"), Src::Daemon);
        assert_eq!(Src::infer_from_target("tdmcp_daemon::ensure"), Src::Daemon);
    }
}
