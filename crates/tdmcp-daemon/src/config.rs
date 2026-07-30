//! Config: CLI > env > RC > defaults.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Resolved runtime config.
#[derive(Debug, Clone)]
pub struct Config {
    /// Listen port.
    pub port: u16,
    /// Data directory.
    pub data_dir: PathBuf,
    /// Bridge package directory.
    pub bridge_dir: PathBuf,
    /// Catalog path.
    pub catalog_path: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
struct RcFile {
    port: Option<u16>,
    data_dir: Option<PathBuf>,
    bridge_dir: Option<PathBuf>,
    catalog: Option<PathBuf>,
}

impl Config {
    /// Load with precedence: CLI overrides > env (already applied by clap) > RC > defaults.
    pub fn load(
        port: Option<u16>,
        data_dir: Option<PathBuf>,
        bridge_dir: Option<PathBuf>,
        catalog: Option<PathBuf>,
    ) -> Result<Self> {
        let default_data = default_data_dir();
        let rc_path = data_dir
            .clone()
            .unwrap_or_else(|| default_data.clone())
            .join("tdmcp-rs.toml");
        let rc = load_rc(&rc_path).unwrap_or_default();

        let data_dir = data_dir.or(rc.data_dir).unwrap_or(default_data);
        let port = port.or(rc.port).unwrap_or(9860);
        let bridge_dir = bridge_dir
            .or(rc.bridge_dir)
            .unwrap_or_else(|| default_bridge_dir(&data_dir));
        let catalog_path = catalog
            .or(rc.catalog)
            .unwrap_or_else(|| default_catalog_path(&data_dir));

        fs::create_dir_all(&data_dir)
            .with_context(|| format!("create data dir {}", data_dir.display()))?;

        Ok(Self {
            port,
            data_dir,
            bridge_dir,
            catalog_path,
        })
    }
}

fn load_rc(path: &PathBuf) -> Result<RcFile> {
    if !path.exists() {
        return Ok(RcFile::default());
    }
    let text = fs::read_to_string(path)?;
    // Minimal TOML-ish: only support JSON for v0 RC to avoid extra dep, or use toml.
    // Prefer JSON sibling if .json; for .toml use simple key parse — add `toml` crate.
    if path.extension().and_then(|e| e.to_str()) == Some("json") {
        return Ok(serde_json::from_str(&text)?);
    }
    // Very small TOML subset via serde is better with toml crate — for now ignore unknown.
    Ok(RcFile::default())
}

fn default_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tdmcp-rs")
}

fn default_bridge_dir(data_dir: &Path) -> PathBuf {
    #[cfg(debug_assertions)]
    {
        let beside = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../bridge");
        if beside.exists() {
            return beside;
        }
    }
    data_dir.join("bridge")
}

fn default_catalog_path(data_dir: &Path) -> PathBuf {
    #[cfg(debug_assertions)]
    {
        let beside =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../diagnostics/catalog.yaml");
        if beside.exists() {
            return beside;
        }
    }
    data_dir.join("diagnostics/catalog.yaml")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn load_respects_explicit_overrides() {
        let data = PathBuf::from("/tmp/tdmcp-test-data");
        let bridge = PathBuf::from("/tmp/tdmcp-test-bridge");
        let catalog = PathBuf::from("/tmp/tdmcp-test-catalog.yaml");
        let cfg = Config::load(
            Some(1234),
            Some(data.clone()),
            Some(bridge.clone()),
            Some(catalog.clone()),
        )
        .expect("load config");
        assert_eq!(cfg.port, 1234);
        assert_eq!(cfg.data_dir, data);
        assert_eq!(cfg.bridge_dir, bridge);
        assert_eq!(cfg.catalog_path, catalog);
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn release_defaults_use_data_dir_only() {
        let data = PathBuf::from("/tmp/tdmcp-default-data");
        assert_eq!(default_bridge_dir(&data), data.join("bridge"));
        assert_eq!(
            default_catalog_path(&data),
            data.join("diagnostics/catalog.yaml")
        );
    }
}
