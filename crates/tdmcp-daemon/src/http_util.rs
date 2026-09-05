//! Shared loopback HTTP helpers for ensure / CLI status / stop.

use anyhow::{bail, Context, Result};
use serde_json::Value;

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(1))
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .context("build bounded admin HTTP client")
}

/// GET a URL; return response body text. Requires a successful status.
pub async fn get_text(url: &str) -> Result<String> {
    let client = client()?;
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    let body = resp.text().await.context("read response body")?;
    if !status.is_success() {
        bail!("GET {url} → HTTP {status}: {body}");
    }
    Ok(body)
}

/// GET JSON from a URL (successful status required).
pub async fn get_json(url: &str) -> Result<Value> {
    let body = get_text(url).await?;
    serde_json::from_str(&body).with_context(|| format!("parse JSON from {url}"))
}

/// POST with an empty body; requires a successful status.
pub async fn post_empty(url: &str) -> Result<()> {
    let client = client()?;
    let resp = client
        .post(url)
        .header(reqwest::header::CONTENT_LENGTH, "0")
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("POST {url} → HTTP {status}: {body}");
    }
    Ok(())
}
