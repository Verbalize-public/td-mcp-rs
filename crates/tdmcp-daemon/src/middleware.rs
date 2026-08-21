//! Auth (Bearer PSK) and loopback-guard middleware for MCP + admin HTTP.

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use tdmcp_diagnostics::codes;

/// Runtime auth knobs copied from config (`auth.mode` / `auth.psk`).
#[derive(Debug, Clone)]
pub struct AuthState {
    /// `"none"` | `"psk"`.
    pub mode: String,
    /// Expected Bearer token when `mode == "psk"`.
    pub psk: String,
}

/// True when the peer address is IPv4/IPv6 loopback.
#[must_use]
pub fn is_loopback(addr: SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Path is under `/admin/federation` (all methods / nested).
#[must_use]
pub fn is_admin_federation_path(path: &str) -> bool {
    path == "/admin/federation" || path.starts_with("/admin/federation/")
}

/// Minimal unauth LAN probe — exempt from Bearer when `mode=psk`.
#[must_use]
pub fn is_federation_status_probe(path: &str) -> bool {
    path == "/admin/federation/status" || path.starts_with("/admin/federation/status/")
}

/// Admin paths allowlisted for remote (non-loopback) access.
#[must_use]
pub fn is_remote_allowed_admin(path: &str) -> bool {
    is_admin_federation_path(path) || path == "/admin/config" || path.starts_with("/admin/config/")
}

/// Loopback-only admin surfaces (shutdown / status / sessions / …).
#[must_use]
pub fn is_loopback_only_admin(path: &str) -> bool {
    if !path.starts_with("/admin/") {
        return false;
    }
    !is_remote_allowed_admin(path)
}

/// Paths that require Bearer when `auth.mode=psk`.
#[must_use]
pub fn requires_psk_auth(path: &str) -> bool {
    if is_federation_status_probe(path) {
        return false;
    }
    path == "/mcp/health"
        || path.starts_with("/mcp/health/")
        || path == "/mcp/rpc"
        || path.starts_with("/mcp/rpc/")
        || path == "/mcp/tools"
        || path.starts_with("/mcp/tools/")
        || is_admin_federation_path(path)
        || path == "/admin/config"
        || path.starts_with("/admin/config/")
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::CONTENT_TYPE, "application/json")],
        json!({
            "ok": false,
            "code": codes::REMOTE_UNAUTHORIZED,
            "message": "missing or invalid Authorization Bearer token",
        })
        .to_string(),
    )
        .into_response()
}

fn forbidden_loopback_response() -> Response {
    (
        StatusCode::FORBIDDEN,
        [(header::CONTENT_TYPE, "application/json")],
        json!({
            "ok": false,
            "error": "admin path requires loopback peer",
        })
        .to_string(),
    )
        .into_response()
}

fn bearer_matches(header_value: Option<&str>, expected: &str) -> bool {
    let Some(raw) = header_value else {
        return false;
    };
    let trimmed = raw.trim();
    let Some(token) = trimmed
        .strip_prefix("Bearer ")
        .or_else(|| trimmed.strip_prefix("bearer "))
    else {
        return false;
    };
    token == expected
}

/// Combined auth + loopback guard. Wire with
/// [`axum::middleware::from_fn_with_state`] before serve, and use
/// `into_make_service_with_connect_info::<SocketAddr>()`.
pub async fn auth_and_loopback(
    State(auth): State<AuthState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();

    if is_loopback_only_admin(&path) && !is_loopback(peer) {
        return forbidden_loopback_response();
    }

    if auth.mode == "psk" && requires_psk_auth(&path) {
        let header = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        if !bearer_matches(header, &auth.psk) {
            return unauthorized_response();
        }
    }

    next.run(request).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn loopback_detects_v4_and_v6() {
        assert!(is_loopback(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            1
        )));
        assert!(is_loopback(SocketAddr::new(
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            1
        )));
        assert!(!is_loopback(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)),
            1
        )));
        assert!(!is_loopback(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            1
        )));
    }

    #[test]
    fn path_classifier_auth() {
        assert!(requires_psk_auth("/mcp/health"));
        assert!(requires_psk_auth("/mcp/rpc"));
        assert!(requires_psk_auth("/mcp/rpc/foo"));
        assert!(requires_psk_auth("/mcp/tools/list"));
        assert!(requires_psk_auth("/admin/federation/register"));
        assert!(requires_psk_auth("/admin/config"));
        assert!(!requires_psk_auth("/admin/federation/status"));
        assert!(!requires_psk_auth("/admin/federation/status/extra"));
        assert!(!requires_psk_auth("/admin/status"));
        assert!(!requires_psk_auth("/admin/shutdown"));
    }

    #[test]
    fn path_classifier_loopback_only() {
        assert!(is_loopback_only_admin("/admin/status"));
        assert!(is_loopback_only_admin("/admin/fleet"));
        assert!(is_loopback_only_admin("/admin/shutdown"));
        assert!(is_loopback_only_admin("/admin/restart"));
        assert!(is_loopback_only_admin("/admin/mcp-sessions"));
        assert!(is_loopback_only_admin("/admin/mcp-sessions/annotate"));
        assert!(!is_loopback_only_admin("/admin/federation/status"));
        assert!(!is_loopback_only_admin("/admin/federation/register"));
        assert!(!is_loopback_only_admin("/admin/config"));
        assert!(!is_loopback_only_admin("/mcp/health"));
    }

    #[test]
    fn bearer_match() {
        assert!(bearer_matches(Some("Bearer secret"), "secret"));
        assert!(bearer_matches(Some("bearer secret"), "secret"));
        assert!(!bearer_matches(Some("Bearer wrong"), "secret"));
        assert!(!bearer_matches(Some("secret"), "secret"));
        assert!(!bearer_matches(None, "secret"));
    }
}
