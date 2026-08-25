//! Wire DTOs — camelCase serde views over the daemon admin API — plus the
//! small display-mapping helpers that turn those records into primitives
//! (level dots/letters, clipped lines, id tails).

use serde::Deserialize;

use crate::theme::{ERR, TEXT_FAINT, WARN};

// ---------------------------------------------------------------------------
// Logs (`/admin/logs`)
// ---------------------------------------------------------------------------

/// One record as returned by `/admin/logs` (camelCase over the wire).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LogRecordView {
    pub(crate) seq: u64,
    pub(crate) ts: String,
    pub(crate) level: String,
    pub(crate) src: String,
    pub(crate) pid: u32,
    pub(crate) target: String,
    pub(crate) msg: String,
    #[serde(default)]
    pub(crate) code: Option<String>,
    #[serde(default)]
    pub(crate) kvs: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LogsResponse {
    pub(crate) records: Vec<LogRecordView>,
    pub(crate) next: u64,
}

/// Dot color per record level (ERR/WRN colored, everything else plain text).
#[must_use]
pub(crate) fn level_color(level: &str) -> eframe::egui::Color32 {
    match level {
        "error" => ERR,
        "warn" => WARN,
        _ => TEXT_FAINT,
    }
}

#[must_use]
pub(crate) fn level_letter(level: &str) -> &'static str {
    match level {
        "trace" => "T",
        "debug" => "D",
        "info" => "I",
        "warn" => "W",
        "error" => "E",
        _ => "?",
    }
}

/// Clip to `max_chars`, char-boundary safe (targets/messages may be UTF-8).
#[must_use]
pub(crate) fn clip_line(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    s.chars().take(max_chars).collect()
}

// ---------------------------------------------------------------------------
// Status / sessions / fleet
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusView {
    pub(crate) version: String,
    pub(crate) pid: u32,
    #[serde(default)]
    pub(crate) mcp_session_count: usize,
    /// Federation role (`standalone` | `master` | `slave`).
    #[serde(default)]
    pub(crate) role: String,
    /// Configured listen IP (`server.bind_address`).
    #[serde(default)]
    pub(crate) bind_address: String,
    /// Local hostname.
    #[serde(default)]
    pub(crate) hostname: String,
    /// Persistent daemon id.
    #[serde(default)]
    pub(crate) daemon_id: String,
    /// Seconds since the daemon process started (absent on old daemons).
    #[serde(default)]
    pub(crate) uptime_secs: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionsView {
    pub(crate) sessions: Vec<SessionRow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionRow {
    pub(crate) id: String,
    pub(crate) client_name: String,
    #[serde(default)]
    pub(crate) client_version: String,
    pub(crate) connected_at: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FleetView {
    pub(crate) processes: Vec<FleetProc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FleetProc {
    pub(crate) pid: u32,
    pub(crate) title: Option<String>,
    pub(crate) bridge: serde_json::Value,
    pub(crate) tasks: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub(crate) cancelled_tasks: Vec<serde_json::Value>,
    #[serde(default)]
    pub(crate) resurrected: bool,
    /// Owning daemon id (aggregated fleet; `None` before federation).
    #[serde(default)]
    pub(crate) daemon_id: Option<String>,
    /// Owning hostname.
    #[serde(default)]
    pub(crate) hostname: Option<String>,
}

/// `/admin/federation/slaves` body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlavesView {
    pub(crate) slaves: Vec<SlaveRow>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlaveRow {
    pub(crate) daemon_id: String,
    #[serde(default)]
    pub(crate) hostname: String,
    /// Slave daemon version — shown once group headers grow a tooltip.
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) version: String,
    #[serde(default)]
    pub(crate) base_url: String,
    #[serde(default)]
    pub(crate) auth_token: String,
    #[serde(default)]
    pub(crate) reachability: String,
    #[serde(default)]
    pub(crate) process_count: usize,
}

/// Minimal `/admin/federation/status` probe (unauth LAN scan oracle).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FederationProbe {
    #[serde(default)]
    pub(crate) role: String,
    #[serde(default)]
    pub(crate) version: String,
    #[serde(default)]
    pub(crate) hostname: String,
    #[serde(default)]
    pub(crate) daemon_id: String,
}

/// One scan hit (a reachable federation daemon on the scanned subnet).
#[derive(Debug, Clone)]
pub(crate) struct ScanHit {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) role: String,
    pub(crate) hostname: String,
    pub(crate) daemon_id: String,
    pub(crate) version: String,
}

/// Slave identity for the settings panel (auth token from the registry).
#[derive(Debug, Clone)]
pub(crate) struct SlaveSettingsTarget {
    pub(crate) daemon_id: String,
    pub(crate) hostname: String,
    pub(crate) base_url: String,
    pub(crate) auth_token: String,
}

/// Which UI triggered the shared subnet scan — keeps the master's "find a
/// slave" scan and a joiner's "find a master" scan from showing each other's
/// stale results, without duplicating the scan state/thread/mpsc plumbing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanPurpose {
    AddSlave,
    JoinMaster,
}

#[must_use]
pub(crate) fn parse_slaves(json: &str) -> Vec<SlaveRow> {
    serde_json::from_str::<SlavesView>(json)
        .map(|v| v.slaves)
        .unwrap_or_default()
}

/// Compact id tail for display (`a1b2…`) from a uuid-shaped string.
#[must_use]
pub(crate) fn id_tail(id: &str) -> String {
    let compact: String = id.chars().filter(|c| *c != '-').collect();
    let tail = if compact.len() > 4 {
        &compact[compact.len() - 4..]
    } else {
        &compact
    };
    format!("{tail}…")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    /// Pins the `/admin/logs` contract (T4.1's `admin.rs` response shape,
    /// `crate::logrecord::Record` in `tdmcp-daemon`) against `LogRecordView`
    /// / `LogsResponse` — no codegen shares the type, so a drift on either
    /// side must fail here (T4.3).
    #[test]
    fn logs_response_round_trips_the_daemon_fixture() {
        let fixture = r#"{
            "records": [
                {
                    "seq": 41,
                    "ts": "2026-01-01T12:00:00.123Z",
                    "level": "warn",
                    "src": "bridge",
                    "pid": 12345,
                    "target": "bridge::tox_callbacks",
                    "msg": "heartbeat pong timeout",
                    "code": "tdmcp.bridge.pong_timeout",
                    "kvs": {"ms": "42"}
                },
                {
                    "seq": 42,
                    "ts": "2026-01-01T12:00:01.000Z",
                    "level": "info",
                    "src": "daemon",
                    "pid": 999,
                    "target": "tdmcp_daemon",
                    "msg": "no code, no kvs"
                }
            ],
            "next": 42
        }"#;
        let parsed: LogsResponse = serde_json::from_str(fixture).expect("parse fixture");
        assert_eq!(parsed.next, 42);
        assert_eq!(parsed.records.len(), 2);

        let first = &parsed.records[0];
        assert_eq!(first.seq, 41);
        assert_eq!(first.level, "warn");
        assert_eq!(first.src, "bridge");
        assert_eq!(first.pid, 12345);
        assert_eq!(first.target, "bridge::tox_callbacks");
        assert_eq!(first.code.as_deref(), Some("tdmcp.bridge.pong_timeout"));
        assert_eq!(first.kvs.get("ms").map(String::as_str), Some("42"));

        let second = &parsed.records[1];
        assert_eq!(second.code, None, "code omitted on the wire when absent");
        assert!(second.kvs.is_empty(), "kvs omitted on the wire when empty");
    }

    #[test]
    fn level_color_and_letter_cover_all_wire_levels() {
        for level in ["trace", "debug", "info", "warn", "error"] {
            let _ = level_color(level);
            assert_ne!(level_letter(level), "?", "missing mapping for {level}");
        }
        assert_eq!(level_letter("not-a-level"), "?");
    }

    #[test]
    fn clip_line_is_char_boundary_safe() {
        let s = "héllo wörld"; // multi-byte chars
        let clipped = clip_line(s, 3);
        assert_eq!(clipped.chars().count(), 3);
        assert_eq!(clipped, "hél");
        assert_eq!(clip_line("short", 100), "short");
    }
}
