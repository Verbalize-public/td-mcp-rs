//! Bounded admin-HTTP helpers for background workers, plus LAN discovery.

use std::time::Duration;

use anyhow::Result;

use crate::wire::ScanHit;

pub(crate) struct PollSnapshot {
    pub status: Result<String, String>,
    pub fleet: Result<String, String>,
    pub sessions: Result<String, String>,
    pub slaves: Result<String, String>,
}

pub(crate) fn poll_snapshot(base: &str, bearer: Option<&str>) -> PollSnapshot {
    let status = http_get_blocking(&format!("{base}/admin/status"), None);
    if let Err(error) = &status {
        return PollSnapshot {
            fleet: Err(error.clone()),
            sessions: Err(error.clone()),
            slaves: Err(error.clone()),
            status,
        };
    }
    let master = status
        .as_ref()
        .ok()
        .and_then(|s| serde_json::from_str::<crate::wire::StatusView>(s).ok())
        .is_some_and(|s| s.role == "master");
    PollSnapshot {
        status,
        fleet: http_get_blocking(&format!("{base}/admin/fleet"), None),
        sessions: http_get_blocking(&format!("{base}/admin/mcp-sessions"), None),
        slaves: if master {
            http_get_blocking(&format!("{base}/admin/federation/slaves"), bearer)
        } else {
            Ok(String::new())
        },
    }
}

pub(crate) fn http_get_blocking(url: &str, bearer: Option<&str>) -> Result<String, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        // Bounded so a dead host costs ~2s on the UI thread, not the OS TCP
        // timeout (~20s on Windows).
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|e| e.to_string())?;
        let mut req = client.get(url);
        if let Some(b) = bearer {
            req = req.bearer_auth(b);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        checked_text(resp).await
    })
}

pub(crate) fn http_post_blocking(
    url: &str,
    bearer: Option<&str>,
    body: Option<&serde_json::Value>,
) -> Result<serde_json::Value, String> {
    http_post_blocking_with_timeout(url, bearer, body, Duration::from_secs(3))
}

pub(crate) fn http_post_blocking_with_timeout(
    url: &str,
    bearer: Option<&str>,
    body: Option<&serde_json::Value>,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| e.to_string())?;
        let mut req = client.post(url);
        if let Some(b) = bearer {
            req = req.bearer_auth(b);
        }
        if let Some(v) = body {
            req = req.json(v);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let body = checked_text(resp).await?;
        let value: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        if value.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
            return Err(response_error(&value)
                .unwrap_or("request rejected")
                .to_owned());
        }
        Ok(value)
    })
}

fn response_error(value: &serde_json::Value) -> Option<&str> {
    value
        .get("error")
        .and_then(serde_json::Value::as_str)
        .or_else(|| value.get("message").and_then(serde_json::Value::as_str))
}

async fn checked_text(response: reqwest::Response) -> Result<String, String> {
    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let parsed = serde_json::from_str::<serde_json::Value>(&body).ok();
        let message = parsed
            .as_ref()
            .and_then(response_error)
            .unwrap_or_else(|| status.canonical_reason().unwrap_or("request failed"));
        return Err(format!("HTTP {status}: {message}"));
    }
    Ok(body)
}

/// Best local (LAN) IPv4 via the UDP connect trick — no packets are sent.
#[must_use]
pub(crate) fn local_ip() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr().ok().map(|a| a.ip().to_string())
}

/// First three octets of an IPv4 (the `/24` prefix); `None` for non-IPv4.
#[must_use]
pub(crate) fn ip_prefix(ip: &str) -> Option<String> {
    let [a, b, c, _] = ip.parse::<std::net::Ipv4Addr>().ok()?.octets();
    Some(format!("{a}.{b}.{c}"))
}

/// Probe `prefix.1..254` on `port` via the unauth `/admin/federation/status` probe.
#[must_use]
pub(crate) fn scan_subnet(prefix: &str, port: u16) -> Vec<ScanHit> {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return Vec::new();
    };
    rt.block_on(async move {
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_millis(400))
            .build()
        else {
            return Vec::new();
        };
        let mut set = tokio::task::JoinSet::new();
        for i in 1..=254u8 {
            let client = client.clone();
            let prefix = prefix.to_owned();
            set.spawn(async move {
                let host = format!("{prefix}.{i}");
                let url = format!("http://{host}:{port}/admin/federation/status");
                let Ok(resp) = client.get(&url).send().await else {
                    return None;
                };
                if !resp.status().is_success() {
                    return None;
                }
                let Ok(v) = resp.json::<serde_json::Value>().await else {
                    return None;
                };
                Some(ScanHit {
                    host,
                    port,
                    role: v
                        .get("role")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("?")
                        .to_owned(),
                    hostname: v
                        .get("hostname")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    daemon_id: v
                        .get("daemonId")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    version: v
                        .get("version")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                })
            });
        }
        let mut results = Vec::new();
        while let Some(joined) = set.join_next().await {
            if let Ok(Some(hit)) = joined {
                results.push(hit);
            }
        }
        results.sort_by_key(|h| h.host.clone());
        results
    })
}

/// Port parsed from an `http://host:port` admin base URL.
#[must_use]
pub(crate) fn port_from_base(base: &str) -> u16 {
    reqwest::Url::parse(base)
        .ok()
        .and_then(|url| url.port_or_known_default())
        .unwrap_or(tdmcp_config::DEFAULT_PORT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_parsing_handles_ipv6_and_rejects_partial_ipv4() {
        assert_eq!(port_from_base("http://[::1]:9860/"), 9860);
        assert_eq!(port_from_base("https://host/"), 443);
        assert_eq!(port_from_base("http://[::1]/"), 80);
        assert_eq!(ip_prefix("192.168.3.4"), Some("192.168.3".into()));
        assert_eq!(ip_prefix("192.168.3"), None);
        assert_eq!(ip_prefix("192.168.3.999"), None);
    }
}
