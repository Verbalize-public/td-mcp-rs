//! Blocking admin-HTTP helpers + LAN subnet scan.
//!
//! Known smell (accepted): every call builds a throwaway current-thread
//! tokio runtime — endpoints are fast and calls are rare; see GUI_MAP.md §3.

use std::time::Duration;

use anyhow::Result;

use crate::wire::ScanHit;

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
        resp.text().await.map_err(|e| e.to_string())
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
        resp.json::<serde_json::Value>()
            .await
            .map_err(|e| e.to_string())
    })
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
    let mut parts = ip.split('.');
    let a: u8 = parts.next()?.parse().ok()?;
    let b: u8 = parts.next()?.parse().ok()?;
    let c: u8 = parts.next()?.parse().ok()?;
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
    let rest = base
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    match rest.rsplit(':').next() {
        Some(p) => p
            .trim_end_matches('/')
            .parse()
            .unwrap_or(tdmcp_config::DEFAULT_PORT),
        None => tdmcp_config::DEFAULT_PORT,
    }
}
