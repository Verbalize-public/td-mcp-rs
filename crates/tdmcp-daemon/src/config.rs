//! Runtime config: CLI/env overrides > TOML config file > defaults.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tdmcp_config::{self as cfgfile, BridgeSection, ConfigFile};

/// Resolved runtime config.
#[derive(Debug, Clone)]
pub struct Config {
    /// Path to the TOML config file that was loaded (or ensured).
    pub config_path: PathBuf,
    /// Listen port.
    pub port: u16,
    /// Data directory.
    pub data_dir: PathBuf,
    /// Bridge package directory.
    pub bridge_dir: PathBuf,
    /// Catalog path.
    pub catalog_path: PathBuf,
    /// Disable idle auto-exit.
    pub keep_alive: bool,
    /// Register OS autostart.
    pub always_on: bool,
    /// Effective headless (CLI `--no-gui` or `show_tray = false`).
    pub no_gui: bool,
    /// Bridge IPC call / heartbeat budgets from `[bridge]`.
    pub bridge: BridgeSection,
}

/// Optional CLI / env overrides passed into [`Config::load`].
#[derive(Debug, Clone, Default)]
pub struct ConfigOverrides {
    /// `--port` / `TDMCP_PORT`.
    pub port: Option<u16>,
    /// `--data-dir` / `TDMCP_DATA_DIR`.
    pub data_dir: Option<PathBuf>,
    /// `--bridge-dir` / `TDMCP_BRIDGE_DIR`.
    pub bridge_dir: Option<PathBuf>,
    /// `--catalog` / `TDMCP_CATALOG`.
    pub catalog: Option<PathBuf>,
    /// `--no-gui` / `TDMCP_NO_GUI`.
    pub no_gui: bool,
}

impl Config {
    /// Load with precedence: CLI/env overrides > config file > defaults.
    ///
    /// Ensures the config file exists (create-if-missing) unless
    /// `TDMCP_CONFIG_PATH` points at a path that should stay untouched until
    /// install --force. Create-if-missing never overwrites an existing file.
    pub fn load(overrides: ConfigOverrides) -> Result<Self> {
        let config_path = cfgfile::default_config_path();
        let _ = cfgfile::ensure_default(&config_path, false)?;
        let file = cfgfile::load(&config_path)?;
        Self::from_file(config_path, file, overrides)
    }

    /// Build resolved config from an already-loaded file + overrides.
    pub fn from_file(
        config_path: PathBuf,
        file: ConfigFile,
        overrides: ConfigOverrides,
    ) -> Result<Self> {
        let default_data = crate::install::default_data_dir();
        let data_dir = overrides
            .data_dir
            .or(file.advanced.data_dir)
            .unwrap_or(default_data);
        let port = overrides.port.unwrap_or(file.server.port);
        let bridge_dir = overrides
            .bridge_dir
            .or(file.advanced.bridge_dir)
            .unwrap_or_else(|| default_bridge_dir(&data_dir));
        let catalog_path = overrides
            .catalog
            .or(file.advanced.catalog_path)
            .unwrap_or_else(|| default_catalog_path(&data_dir));
        let no_gui = overrides.no_gui || !file.daemon.show_tray;

        fs::create_dir_all(&data_dir)
            .with_context(|| format!("create data dir {}", data_dir.display()))?;

        Ok(Self {
            config_path,
            port,
            data_dir,
            bridge_dir,
            catalog_path,
            keep_alive: file.daemon.keep_alive,
            always_on: file.daemon.always_on,
            no_gui,
            bridge: file.bridge,
        })
    }
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
    use tempfile::tempdir;

    #[test]
    fn overrides_win_over_file() {
        let dir = tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        cfgfile::ensure_default(&config_path, true).expect("seed");
        let file = cfgfile::load(&config_path).expect("load file");
        let data = dir.path().join("data");
        let bridge = dir.path().join("bridge");
        let catalog = dir.path().join("catalog.yaml");
        let cfg = Config::from_file(
            config_path,
            file,
            ConfigOverrides {
                port: Some(1234),
                data_dir: Some(data.clone()),
                bridge_dir: Some(bridge.clone()),
                catalog: Some(catalog.clone()),
                no_gui: true,
            },
        )
        .expect("from_file");
        assert_eq!(cfg.port, 1234);
        assert_eq!(cfg.data_dir, data);
        assert_eq!(cfg.bridge_dir, bridge);
        assert_eq!(cfg.catalog_path, catalog);
        assert!(cfg.no_gui);
    }

    #[test]
    fn keep_alive_from_file() {
        let dir = tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        cfgfile::ensure_default(&config_path, true).expect("seed");
        let mut file = cfgfile::load(&config_path).expect("load");
        file.daemon.keep_alive = true;
        file.daemon.always_on = true;
        file.daemon.show_tray = false;
        cfgfile::save(&config_path, &file).expect("save");
        let cfg = Config::from_file(
            config_path,
            file,
            ConfigOverrides {
                data_dir: Some(dir.path().join("data")),
                ..Default::default()
            },
        )
        .expect("from_file");
        assert!(cfg.keep_alive);
        assert!(cfg.always_on);
        assert!(cfg.no_gui);
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
