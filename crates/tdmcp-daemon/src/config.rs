//! Runtime config: CLI/env overrides > TOML config file > defaults.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tdmcp_config::DialogsSection;
use tdmcp_config::{self as cfgfile, BridgeSection, ConfigFile};

/// Env override for the bridge listener host (beats `[bridge] host`).
pub const IPC_HOST_ENV: &str = "TDMCP_IPC_HOST";
/// Env override for the bridge listener port (beats `[bridge] port`).
pub const IPC_PORT_ENV: &str = "TDMCP_IPC_PORT";

/// Resolved runtime config.
#[derive(Debug, Clone)]
pub struct Config {
    /// File snapshot before CLI/environment overrides, used by live settings.
    pub file: ConfigFile,
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
    /// Resolved bridge listener host (env → config → default, loopback-only).
    pub bridge_host: String,
    /// Resolved bridge listener port (env → config → default).
    pub bridge_port: u16,
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
    /// `[dialogs]` switches (watcher + interception).
    pub dialogs: DialogsSection,
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
        cfgfile::validate(&file)?;
        let _ = cfgfile::ensure_daemon_id(&config_path, &mut file)?;
        let file_snapshot = file.clone();

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

        // Bridge endpoint resolution happens here (composition root) so
        // env/config precedence and loopback validation fail at startup
        // rather than inside the IPC crate.
        let (bridge_host, bridge_port) = resolve_bridge_endpoint(
            std::env::var(IPC_HOST_ENV).ok().as_deref(),
            std::env::var(IPC_PORT_ENV).ok().as_deref(),
            &file.bridge,
        )?;

        let logging_dir = file
            .logging
            .dir
            .clone()
            .unwrap_or_else(|| data_dir.join("logs"));

        fs::create_dir_all(&data_dir)
            .with_context(|| format!("create data dir {}", data_dir.display()))?;

        Ok(Self {
            file: file_snapshot,
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
            bridge_host,
            bridge_port,
            logging_dir,
            logging_filter: file.logging.filter,
            logging_console_level: file.logging.console_level,
            // Clamp degenerate zeros so rotation/sweep always keep something.
            logging_max_files: file.logging.max_files.max(1),
            logging_retention_days: file.logging.retention_days.max(1),
            daemon_bin: file.advanced.daemon_bin,
            dialogs: file.dialogs,
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

/// Resolve the bridge bind endpoint: env → config → default (T-2), rejecting
/// non-loopback hosts (T-4) and port 0 (T-3 forbids port hopping).
///
/// Arguments are pre-read env values so precedence is unit-testable without
/// mutating process-global env.
fn resolve_bridge_endpoint(
    env_host: Option<&str>,
    env_port: Option<&str>,
    bridge: &BridgeSection,
) -> Result<(String, u16)> {
    let host = env_host
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let host = bridge.host.trim();
            if host.is_empty() {
                cfgfile::DEFAULT_BRIDGE_HOST.to_owned()
            } else {
                host.to_owned()
            }
        });
    if !is_loopback_host(&host) {
        bail!(
            "bridge host {host:?} is not loopback — the bridge port must bind \
             127.0.0.1/::1 only in v0; fix [bridge] host or {IPC_HOST_ENV}"
        );
    }
    let port = match env_port.map(str::trim).filter(|p| !p.is_empty()) {
        Some(raw) => raw
            .parse::<u16>()
            .with_context(|| format!("parse {IPC_PORT_ENV} {raw:?}"))?,
        None if bridge.port == 0 => bail!(
            "bridge port 0 would bind a random port — the bridge needs a \
             deterministic endpoint; fix [bridge] port or {IPC_PORT_ENV}"
        ),
        None => bridge.port,
    };
    Ok((host, port))
}

/// Read the env overrides and resolve against `bridge` (production path).
pub fn resolve_bridge_endpoint_from_env(bridge: &BridgeSection) -> Result<(String, u16)> {
    resolve_bridge_endpoint(
        std::env::var(IPC_HOST_ENV).ok().as_deref(),
        std::env::var(IPC_PORT_ENV).ok().as_deref(),
        bridge,
    )
}

/// Loopback = `localhost` or an IP inside the loopback ranges (T-4).
fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Env vars are process-global; serialize tests that touch them.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn bridge_endpoint_env_beats_config_beats_default() {
        // Defaults.
        let (host, port) =
            resolve_bridge_endpoint(None, None, &BridgeSection::default()).expect("defaults");
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, cfgfile::DEFAULT_BRIDGE_PORT);
        // Config beats default.
        let configured = BridgeSection {
            host: "127.0.0.2".to_owned(),
            port: 9999,
            ..Default::default()
        };
        let (host, port) = resolve_bridge_endpoint(None, None, &configured).expect("config");
        assert_eq!(host, "127.0.0.2");
        assert_eq!(port, 9999);
        // Env beats config.
        let (host, port) =
            resolve_bridge_endpoint(Some("127.0.0.3"), Some("7000"), &configured).expect("env");
        assert_eq!(host, "127.0.0.3");
        assert_eq!(port, 7000);
    }

    #[test]
    fn bridge_endpoint_rejects_non_loopback_host() {
        for host in ["0.0.0.0", "192.168.1.9", "example.com"] {
            let bridge = BridgeSection {
                host: host.to_owned(),
                ..Default::default()
            };
            let err = resolve_bridge_endpoint(None, None, &bridge).expect_err(host);
            assert!(
                err.to_string().contains("loopback"),
                "{host:?} must be rejected as non-loopback, got: {err}"
            );
        }
        // Env override cannot smuggle a non-loopback host past validation.
        let err = resolve_bridge_endpoint(Some("0.0.0.0"), None, &BridgeSection::default())
            .expect_err("env host");
        assert!(err.to_string().contains("loopback"), "got: {err}");
    }

    #[test]
    fn bridge_endpoint_rejects_invalid_port() {
        let err = resolve_bridge_endpoint(None, Some("not-a-port"), &BridgeSection::default())
            .expect_err("env port");
        assert!(err.to_string().contains(IPC_PORT_ENV), "got: {err}");
        let bridge = BridgeSection {
            port: 0,
            ..Default::default()
        };
        let err = resolve_bridge_endpoint(None, None, &bridge).expect_err("port 0");
        assert!(
            err.to_string().contains("deterministic"),
            "port 0 must be refused, got: {err}"
        );
    }

    #[test]
    fn bridge_endpoint_env_names_match_runtime_resolution() {
        let _guard = env_guard();
        // Prove the production wrapper reads exactly TDMCP_IPC_HOST/PORT.
        std::env::set_var(IPC_HOST_ENV, "127.0.0.4");
        std::env::set_var(IPC_PORT_ENV, "7001");
        let resolved = resolve_bridge_endpoint_from_env(&BridgeSection::default());
        std::env::remove_var(IPC_HOST_ENV);
        std::env::remove_var(IPC_PORT_ENV);
        let (host, port) = resolved.expect("env resolution");
        assert_eq!(host, "127.0.0.4");
        assert_eq!(port, 7001);
    }

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
