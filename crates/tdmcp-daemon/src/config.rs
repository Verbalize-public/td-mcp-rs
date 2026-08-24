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
    /// Bind IP string from `[server].bind_address`.
    pub bind_address: String,
    /// `[auth].mode` (`none` | `psk`).
    pub auth_mode: String,
    /// `[auth].psk` (Bearer token when mode is psk).
    pub auth_psk: String,
    /// `[federation].role` (`standalone` | `master` | `slave`).
    pub federation_role: String,
    /// Persistent `[federation].daemon_id`.
    pub daemon_id: String,
    /// `[federation].master_url` when role is slave.
    pub master_url: String,
    /// `[federation].master_psk` when role is slave.
    pub master_psk: String,
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
    /// Resolved `[logging]` directory (`[logging].dir` > `data_dir/logs`).
    pub logging_dir: PathBuf,
    /// EnvFilter string for the file layer; None => RUST_LOG => built-in default.
    pub logging_filter: Option<String>,
    /// Separate EnvFilter for stderr; None => current console defaults.
    pub logging_console_level: Option<String>,
    /// Daily rotated log files kept on disk.
    pub logging_max_files: u32,
    /// Log sweep threshold in days.
    pub logging_retention_days: u32,
    /// Path to the installed daemon binary (auto-set by `install`).
    /// When `Some`, spawn / restart / autostart use this path instead of `current_exe()`.
    pub daemon_bin: Option<PathBuf>,
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
        mut file: ConfigFile,
        overrides: ConfigOverrides,
    ) -> Result<Self> {
        cfgfile::validate_remote_auth(&file)?;
        let _ = cfgfile::ensure_daemon_id(&config_path, &mut file)?;

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

        let logging_dir = file
            .logging
            .dir
            .clone()
            .unwrap_or_else(|| data_dir.join("logs"));

        fs::create_dir_all(&data_dir)
            .with_context(|| format!("create data dir {}", data_dir.display()))?;

        Ok(Self {
            config_path,
            port,
            bind_address: file.server.bind_address,
            auth_mode: file.auth.mode,
            auth_psk: file.auth.psk,
            federation_role: file.federation.role,
            daemon_id: file.federation.daemon_id,
            master_url: file.federation.master_url,
            master_psk: file.federation.master_psk,
            data_dir,
            bridge_dir,
            catalog_path,
            keep_alive: file.daemon.keep_alive,
            always_on: file.daemon.always_on,
            no_gui,
            bridge: file.bridge,
            logging_dir,
            logging_filter: file.logging.filter,
            logging_console_level: file.logging.console_level,
            // Clamp degenerate zeros so rotation/sweep always keep something.
            logging_max_files: file.logging.max_files.max(1),
            logging_retention_days: file.logging.retention_days.max(1),
            daemon_bin: file.advanced.daemon_bin,
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
    fn logging_defaults_and_clamps() {
        let dir = tempdir().expect("tempdir");
        let config_path = dir.path().join("config.toml");
        cfgfile::ensure_default(&config_path, true).expect("seed");
        let mut file = cfgfile::load(&config_path).expect("load");
        file.logging.max_files = 0;
        file.logging.retention_days = 0;
        let data = dir.path().join("data");
        let cfg = Config::from_file(
            config_path,
            file,
            ConfigOverrides {
                data_dir: Some(data.clone()),
                ..Default::default()
            },
        )
        .expect("from_file");
        assert_eq!(cfg.logging_dir, data.join("logs"));
        assert_eq!(cfg.logging_filter, None);
        assert_eq!(cfg.logging_console_level, None);
        assert_eq!(cfg.logging_max_files, 1);
        assert_eq!(cfg.logging_retention_days, 1);
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
