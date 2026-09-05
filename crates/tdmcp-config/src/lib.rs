//! td-mcp-rs TOML configuration file.
//!
//! Default path: `{config_dir}/tdmcp-rs/config.toml` (OS-standard config dir).
//! The curated template lives in `assets/default.toml` and is embedded at
//! compile time.

#![warn(missing_docs)]

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use toml_edit::{value, DocumentMut, Item, Table};

/// Embedded default / example config (commented template).
pub const DEFAULT_TOML: &str = include_str!("../assets/default.toml");

/// Env var that overrides [`default_config_path`] (tests / isolation).
pub const CONFIG_PATH_ENV: &str = "TDMCP_CONFIG_PATH";

/// Default HTTP listen port (MCP + admin).
pub const DEFAULT_PORT: u16 = 9860;

/// Default TCP listen port for the daemon↔bridge transport (loopback only).
pub const DEFAULT_BRIDGE_PORT: u16 = 9861;

/// Default TCP bind host for the daemon↔bridge transport (loopback only).
pub const DEFAULT_BRIDGE_HOST: &str = "127.0.0.1";

/// App directory name under OS config/data dirs.
pub const APP_DIR_NAME: &str = "tdmcp-rs";

/// Full on-disk config document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigFile {
    /// Listen / MCP server settings.
    pub server: ServerSection,
    /// Incoming request authentication.
    pub auth: AuthSection,
    /// Federation / master–slave role.
    pub federation: FederationSection,
    /// Daemon lifecycle settings.
    pub daemon: DaemonSection,
    /// Log sink / rotation / retention settings.
    pub logging: LoggingSection,
    /// Bridge IPC call / heartbeat budgets.
    pub bridge: BridgeSection,
    /// Optional path overrides.
    pub advanced: AdvancedSection,
    /// Official `toeexpand`/`toecollapse` pinning (v2 project I/O).
    pub official_tools: OfficialToolsSection,
    /// OS-dialog detection / interception switches.
    pub dialogs: DialogsSection,
    /// Template / new-project defaults.
    pub project: ProjectSection,
    /// Palette component library discovery + probe blacklist.
    pub palette: PaletteSection,
}

/// `[server]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSection {
    /// HTTP listen port (MCP + admin).
    pub port: u16,
    /// Bind IP (`127.0.0.1` loopback, `0.0.0.0` for remote).
    pub bind_address: String,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            bind_address: "127.0.0.1".to_owned(),
        }
    }
}

/// `[auth]` table — Bearer PSK for non-loopback / optional local auth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthSection {
    /// `"none"` | `"psk"`.
    pub mode: String,
    /// Shared secret for `Authorization: Bearer <psk>` when `mode = "psk"`.
    pub psk: String,
}

impl Default for AuthSection {
    fn default() -> Self {
        Self {
            mode: "none".to_owned(),
            psk: String::new(),
        }
    }
}

/// `[federation]` table — standalone / master / slave identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FederationSection {
    /// `"standalone"` | `"master"` | `"slave"`.
    pub role: String,
    /// Persistent UUID generated on first start; never changes once set.
    pub daemon_id: String,
    /// Master base URL when `role = "slave"` (e.g. `http://192.168.1.100:9860`).
    pub master_url: String,
    /// PSK to present to the master (the master’s `auth.psk`).
    pub master_psk: String,
}

impl Default for FederationSection {
    fn default() -> Self {
        Self {
            role: "standalone".to_owned(),
            daemon_id: String::new(),
            master_url: String::new(),
            master_psk: String::new(),
        }
    }
}

/// `[daemon]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonSection {
    /// Disable idle auto-exit when true.
    pub keep_alive: bool,
    /// Register OS autostart when true.
    pub always_on: bool,
    /// Show tray dashboard when true (gui builds).
    pub show_tray: bool,
}

impl Default for DaemonSection {
    fn default() -> Self {
        Self {
            keep_alive: true,
            always_on: false,
            show_tray: true,
        }
    }
}

/// `[logging]` table — rotating JSONL sink and retention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingSection {
    /// Override log directory; unset → `{data_dir}/logs`.
    pub dir: Option<PathBuf>,
    /// EnvFilter string for the file layer; unset → `RUST_LOG` → built-in default.
    pub filter: Option<String>,
    /// Separate EnvFilter for the stderr console; unset → current defaults.
    pub console_level: Option<String>,
    /// Daily rotated files kept.
    pub max_files: u32,
    /// Files older than this many days are swept at startup and every 24 h.
    pub retention_days: u32,
}

impl Default for LoggingSection {
    fn default() -> Self {
        Self {
            dir: None,
            filter: None,
            console_level: None,
            max_files: 14,
            retention_days: 30,
        }
    }
}

/// `[bridge]` table — TCP endpoint, IPC call budgets, and idle heartbeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BridgeSection {
    /// Bind host for the bridge listener. Loopback only in v0 (see T-4).
    pub host: String,
    /// Bind port for the bridge listener.
    pub port: u16,
    /// Default per-call wait for `ping` / `inspect` / `capture` (seconds).
    pub call_timeout_secs: u64,
    /// Per-call wait for `execute_python` / `mutate_nodes` (seconds).
    pub script_timeout_secs: u64,
    /// Idle heartbeat ping interval (seconds).
    pub heartbeat_interval_secs: u64,
    /// Max wait for a heartbeat pong (seconds).
    pub pong_timeout_secs: u64,
    /// Tear down after this many seconds with no inbound framed traffic.
    pub idle_dead_secs: u64,
}

impl Default for BridgeSection {
    fn default() -> Self {
        Self {
            host: DEFAULT_BRIDGE_HOST.to_owned(),
            port: DEFAULT_BRIDGE_PORT,
            call_timeout_secs: 45,
            script_timeout_secs: 120,
            heartbeat_interval_secs: 5,
            pong_timeout_secs: 8,
            idle_dead_secs: 20,
        }
    }
}

/// `[advanced]` table — optional path overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvancedSection {
    /// Override data directory.
    pub data_dir: Option<PathBuf>,
    /// Override bridge package directory.
    pub bridge_dir: Option<PathBuf>,
    /// Override diagnostics catalog path.
    pub catalog_path: Option<PathBuf>,
    /// Path to the installed daemon binary (set automatically by `tdmcp-daemon install`).
    /// Used for spawn / restart / autostart instead of `current_exe()`.
    pub daemon_bin: Option<PathBuf>,
}

/// `[official_tools]` table — pin Derivative's expand/collapse tools.
///
/// All optional; absence triggers env (`TDMCP_TOEEXPAND` / `TDMCP_TOECOLLAPSE`
/// / `TDMCP_TOUCHDESIGNER_EXE`) then Program Files scan. Setting exactly one
/// of expand/collapse is a configuration error (XOR-pair rule).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OfficialToolsSection {
    /// Pin one install via its TouchDesigner.exe path.
    pub td_exe: Option<PathBuf>,
    /// Explicit toeexpand binary.
    pub expand_path: Option<PathBuf>,
    /// Explicit toecollapse binary.
    pub collapse_path: Option<PathBuf>,
    /// Linux only: Wine binary used to run `.exe` tools and TouchDesigner
    /// itself (a bare name resolved on `PATH`, or an absolute path — a
    /// Lutris/Bottles/CrossOver wrapper script works too). Unset = `"wine"`.
    pub wine_exe: Option<String>,
    /// Linux only: explicit Wine prefix to scan for a TouchDesigner install,
    /// for a layout `td_installs` doesn't autodetect (Steam Proton
    /// `compatdata`, a CrossOver bottle, …). Unset = autodetect
    /// (`$WINEPREFIX`, `~/.wine`, `~/.local/share/wineprefixes/*`).
    pub wine_prefix: Option<PathBuf>,
}

/// `[dialogs]` table — daemon-side popup watcher + interception gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DialogsSection {
    /// Master switch: popup watcher + `dialogs` tool (Windows and macOS).
    pub enabled: bool,
    /// Fail bridged tool calls fast while a modal blocks the TD main thread.
    pub intercept: bool,
    /// Watcher cadence in milliseconds.
    pub poll_ms: u64,
}

impl Default for DialogsSection {
    fn default() -> Self {
        Self {
            enabled: true,
            intercept: true,
            poll_ms: 1000,
        }
    }
}

/// `[project]` table — template for `spawn_td` create-new.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSection {
    /// Path to the template `.toe` used by `spawn_td` `createIfMissing`.
    /// `None` → `{data_dir}/template.toe` (shipped fallback).
    pub template_path: Option<PathBuf>,
}

/// `[palette]` table — TouchDesigner Palette component library.
///
/// The builtin root is discovered from the TD install; this table only covers
/// the two things discovery cannot know: where the *user's* palette folder is,
/// and which components must never be probed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaletteSection {
    /// User palette folder. `None` → `{documents}/Derivative/Palette`.
    pub user_root: Option<PathBuf>,
    /// Index + card store. `None` → `{data_dir}/palette`.
    pub store_dir: Option<PathBuf>,
    /// Id globs never probed. Seeded with components that open sockets or
    /// expect absent hardware on load — probing those can wedge TD.
    pub ignore: Vec<String>,
}

/// Palette ignore globs applied to a freshly created index.
pub const DEFAULT_PALETTE_IGNORE: &[&str] = &[
    "builtin:TDAbleton/*",
    "builtin:TDBitwig/*",
    "builtin:TDSynchro/*",
    "builtin:TDVR/*",
    "builtin:MetaQuest/*",
    "builtin:Vive/*",
    "builtin:WebRTC/*",
];

impl Default for PaletteSection {
    fn default() -> Self {
        Self {
            user_root: None,
            store_dir: None,
            ignore: DEFAULT_PALETTE_IGNORE
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
        }
    }
}

/// Field descriptions shared by docs, GUI tooltips, and the default template.
#[derive(Debug, Clone, Copy)]
pub struct FieldDesc {
    /// TOML key path (e.g. `server.port`).
    pub key: &'static str,
    /// Short UI label.
    pub label: &'static str,
    /// Longer help text.
    pub help: &'static str,
}

/// Curated field table for Settings UI / docs.
pub const FIELD_DESCS: &[FieldDesc] = &[
    FieldDesc {
        key: "official_tools.td_exe",
        label: "TouchDesigner.exe",
        help: "Pin one install for project I/O; tools are expected beside it. Empty = auto-discover.",
    },
    FieldDesc {
        key: "official_tools.expand_path",
        label: "toeexpand path",
        help: "Explicit toeexpand binary (must be set together with collapse path).",
    },
    FieldDesc {
        key: "dialogs.enabled",
        label: "Dialog watcher",
        help: "Detect TouchDesigner popups daemon-side and fill windowStatus.",
    },
    FieldDesc {
        key: "dialogs.intercept",
        label: "Popup interception",
        help: "Fail bridged tool calls fast with tdmcp.dialog.blocking while a modal is open.",
    },
    FieldDesc {
        key: "dialogs.poll_ms",
        label: "Poll interval (ms)",
        help: "How often the dialog watcher samples registered pids.",
    },
    FieldDesc {
        key: "official_tools.collapse_path",
        label: "toecollapse path",
        help: "Explicit toecollapse binary (must be set together with expand path).",
    },
    FieldDesc {
        key: "official_tools.wine_exe",
        label: "Wine binary (Linux)",
        help: "Wine binary/wrapper used to run TouchDesigner and its tools. Empty = \"wine\".",
    },
    FieldDesc {
        key: "official_tools.wine_prefix",
        label: "Wine prefix (Linux)",
        help: "Explicit prefix to scan when auto-detection ($WINEPREFIX, ~/.wine, ~/.local/share/wineprefixes/*) misses your install (e.g. Steam Proton, CrossOver).",
    },
    FieldDesc {
        key: "server.port",
        label: "Port",
        help: "HTTP listen port for MCP and admin (default 9860).",
    },
    FieldDesc {
        key: "server.bind_address",
        label: "Bind address",
        help: "Listen IP (default 127.0.0.1). Use 0.0.0.0 for LAN/remote reachability.",
    },
    FieldDesc {
        key: "auth.mode",
        label: "Auth mode",
        help: "Incoming auth: none (default, no token needed) or psk (require a Bearer token).",
    },
    FieldDesc {
        key: "auth.psk",
        label: "Auth PSK",
        help: "Shared secret for Authorization: Bearer when auth.mode=psk.",
    },
    FieldDesc {
        key: "federation.role",
        label: "Federation role",
        help: "standalone (default), master (accept slaves), or slave (register + push fleet).",
    },
    FieldDesc {
        key: "federation.daemon_id",
        label: "Daemon ID",
        help: "Persistent UUID for this daemon; auto-generated on first start.",
    },
    FieldDesc {
        key: "federation.master_url",
        label: "Master URL",
        help: "When role=slave: master base URL (http://host:port).",
    },
    FieldDesc {
        key: "federation.master_psk",
        label: "Master PSK",
        help: "When role=slave: Bearer token matching the master’s auth.psk.",
    },
    FieldDesc {
        key: "daemon.keep_alive",
        label: "Keep alive",
        help: "Disable auto-shutdown when no MCP sessions and no TD bridges are connected.",
    },
    FieldDesc {
        key: "daemon.always_on",
        label: "Always on",
        help: "Start the daemon automatically at user login (OS autostart).",
    },
    FieldDesc {
        key: "daemon.show_tray",
        label: "Show tray",
        help: "Show the system-tray dashboard (gui builds). CLI --no-gui still forces headless.",
    },
    FieldDesc {
        key: "bridge.host",
        label: "Bridge host",
        help: "Loopback bind host for the daemon↔bridge TCP listener (default 127.0.0.1).",
    },
    FieldDesc {
        key: "bridge.port",
        label: "Bridge port",
        help: "TCP port of the daemon↔bridge listener (default 9861).",
    },
    FieldDesc {
        key: "bridge.call_timeout_secs",
        label: "Call timeout",
        help: "Seconds to wait for ping / inspect / capture responses (default 45).",
    },
    FieldDesc {
        key: "bridge.script_timeout_secs",
        label: "Script timeout",
        help: "Seconds to wait for execute_python / mutate_nodes (default 120).",
    },
    FieldDesc {
        key: "bridge.heartbeat_interval_secs",
        label: "Heartbeat interval",
        help: "Seconds between idle bridge ping probes (default 5).",
    },
    FieldDesc {
        key: "bridge.pong_timeout_secs",
        label: "Pong timeout",
        help: "Seconds to wait for a heartbeat pong before treating the peer as dead (default 8).",
    },
    FieldDesc {
        key: "bridge.idle_dead_secs",
        label: "Idle dead",
        help: "Seconds of inbound silence before tearing down a bridge session (default 20).",
    },
    FieldDesc {
        key: "advanced.data_dir",
        label: "Data dir",
        help: "Optional override for the install/data directory.",
    },
    FieldDesc {
        key: "advanced.bridge_dir",
        label: "Bridge dir",
        help: "Optional override for the Python bridge package directory.",
    },
    FieldDesc {
        key: "advanced.catalog_path",
        label: "Catalog path",
        help: "Optional override for diagnostics/catalog.yaml.",
    },
    FieldDesc {
        key: "advanced.daemon_bin",
        label: "Daemon bin",
        help: "Path to the installed daemon binary (auto-set by `install`; used for spawn / restart / autostart).",
    },
    FieldDesc {
        key: "project.template_path",
        label: "Template .toe",
        help: "Template .toe for spawn_td createIfMissing. Empty = {data_dir}/template.toe (shipped fallback).",
    },
    FieldDesc {
        key: "palette.user_root",
        label: "User palette folder",
        help: "Your own .tox palette folder. Empty = {documents}/Derivative/Palette.",
    },
    FieldDesc {
        key: "palette.store_dir",
        label: "Palette store",
        help: "Where the palette index + component cards live. Empty = {data_dir}/palette.",
    },
];

/// Default config file path (`{config_dir}/tdmcp-rs/config.toml`).
///
/// Honours [`CONFIG_PATH_ENV`] when set.
#[must_use]
pub fn default_config_path() -> PathBuf {
    if let Ok(p) = std::env::var(CONFIG_PATH_ENV) {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
        .join("config.toml")
}

/// Write the embedded default template when missing, or always when `force`.
///
/// Returns `true` when the file was (re)written.
pub fn ensure_default(path: &Path, force: bool) -> Result<bool> {
    if !force && path.is_file() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create config dir {}", parent.display()))?;
    }
    fs::write(path, DEFAULT_TOML)
        .with_context(|| format!("write default config {}", path.display()))?;
    Ok(true)
}

/// Load config from `path`. Missing file → [`ConfigFile::default`].
pub fn load(path: &Path) -> Result<ConfigFile> {
    if !path.is_file() {
        tracing::debug!(path = %path.display(), "config file missing — using defaults");
        let file = ConfigFile::default();
        apply_wine_env(&file);
        return Ok(file);
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    let file: ConfigFile = toml_edit::de::from_str(&text)
        .with_context(|| format!("parse config {}", path.display()))?;
    tracing::debug!(path = %path.display(), "config loaded");
    apply_wine_env(&file);
    Ok(file)
}

/// Linux only: promote `[official_tools] wine_exe`/`wine_prefix` into
/// `TDMCP_WINE_EXE`/`TDMCP_WINE_PREFIX` process env so `tdmcp-projectio`'s
/// Wine invocation and Wine-prefix scan — both plain env readers, matching
/// every other `TDMCP_*` official-tool override — pick them up without
/// threading config through every call site. No-op when unset; a no-op on
/// Windows/macOS regardless.
#[cfg(all(not(windows), not(target_os = "macos")))]
fn apply_wine_env(cfg: &ConfigFile) {
    if let Some(exe) = &cfg.official_tools.wine_exe {
        std::env::set_var("TDMCP_WINE_EXE", exe);
    }
    if let Some(prefix) = &cfg.official_tools.wine_prefix {
        std::env::set_var("TDMCP_WINE_PREFIX", prefix);
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn apply_wine_env(_cfg: &ConfigFile) {}

/// True when `bind_address` is IPv4/IPv6 loopback (`127.0.0.1` or `::1`).
#[must_use]
pub fn is_loopback_bind(bind_address: &str) -> bool {
    matches!(bind_address.trim(), "127.0.0.1" | "::1")
}

/// PSK is optional even on a non-loopback bind (local-network federation is
/// meant to work with zero setup); this only catches the internally-broken
/// combination of explicitly choosing `auth.mode = "psk"` with no secret set.
pub fn validate_remote_auth(file: &ConfigFile) -> Result<()> {
    if file.auth.mode == "psk" && file.auth.psk.trim().is_empty() {
        anyhow::bail!("auth.mode = \"psk\" requires a non-empty auth.psk");
    }
    Ok(())
}

/// Ensure [`FederationSection::daemon_id`] is set; generate a UUIDv4 and save when empty.
///
/// Returns `true` when the file was written.
pub fn ensure_daemon_id(path: &Path, cfg: &mut ConfigFile) -> Result<bool> {
    if !cfg.federation.daemon_id.trim().is_empty() {
        return Ok(false);
    }
    cfg.federation.daemon_id = uuid::Uuid::new_v4().to_string();
    save(path, cfg)?;
    Ok(true)
}

/// Save `cfg` into `path`, preserving comments/formatting when possible.
pub fn save(path: &Path, cfg: &ConfigFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create config dir {}", parent.display()))?;
    }

    let existing = if path.is_file() {
        fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?
    } else {
        DEFAULT_TOML.to_owned()
    };

    let mut doc = existing
        .parse::<DocumentMut>()
        .unwrap_or_else(|_| DocumentMut::new());

    ensure_table(&mut doc, "server");
    ensure_table(&mut doc, "auth");
    ensure_table(&mut doc, "federation");
    ensure_table(&mut doc, "daemon");
    ensure_table(&mut doc, "logging");
    ensure_table(&mut doc, "bridge");
    ensure_table(&mut doc, "advanced");
    ensure_table(&mut doc, "official_tools");
    ensure_table(&mut doc, "dialogs");
    ensure_table(&mut doc, "project");
    ensure_table(&mut doc, "palette");

    doc["dialogs"]["enabled"] = value(cfg.dialogs.enabled);
    doc["dialogs"]["intercept"] = value(cfg.dialogs.intercept);
    doc["dialogs"]["poll_ms"] = value(cfg.dialogs.poll_ms as i64);
    ensure_table(&mut doc, "dialogs");

    doc["server"]["port"] = value(i64::from(cfg.server.port));
    doc["server"]["bind_address"] = value(cfg.server.bind_address.as_str());
    doc["auth"]["mode"] = value(cfg.auth.mode.as_str());
    doc["auth"]["psk"] = value(cfg.auth.psk.as_str());
    doc["federation"]["role"] = value(cfg.federation.role.as_str());
    doc["federation"]["daemon_id"] = value(cfg.federation.daemon_id.as_str());
    doc["federation"]["master_url"] = value(cfg.federation.master_url.as_str());
    doc["federation"]["master_psk"] = value(cfg.federation.master_psk.as_str());
    doc["daemon"]["keep_alive"] = value(cfg.daemon.keep_alive);
    doc["daemon"]["always_on"] = value(cfg.daemon.always_on);
    doc["daemon"]["show_tray"] = value(cfg.daemon.show_tray);

    doc["bridge"]["host"] = value(cfg.bridge.host.as_str());
    doc["bridge"]["port"] = value(i64::from(cfg.bridge.port));
    doc["bridge"]["call_timeout_secs"] = value(cfg.bridge.call_timeout_secs as i64);
    doc["bridge"]["script_timeout_secs"] = value(cfg.bridge.script_timeout_secs as i64);
    doc["bridge"]["heartbeat_interval_secs"] = value(cfg.bridge.heartbeat_interval_secs as i64);
    doc["bridge"]["pong_timeout_secs"] = value(cfg.bridge.pong_timeout_secs as i64);
    doc["bridge"]["idle_dead_secs"] = value(cfg.bridge.idle_dead_secs as i64);

    doc["logging"]["max_files"] = value(cfg.logging.max_files as i64);
    doc["logging"]["retention_days"] = value(cfg.logging.retention_days as i64);
    let dir_str = cfg
        .logging
        .dir
        .as_deref()
        .map(|p| p.to_string_lossy().into_owned());
    set_optional_str(&mut doc["logging"], "dir", dir_str.as_deref());
    set_optional_str(&mut doc["logging"], "filter", cfg.logging.filter.as_deref());
    set_optional_str(
        &mut doc["logging"],
        "console_level",
        cfg.logging.console_level.as_deref(),
    );

    set_optional_path(
        &mut doc["advanced"],
        "data_dir",
        cfg.advanced.data_dir.as_ref(),
    );
    set_optional_path(
        &mut doc["advanced"],
        "bridge_dir",
        cfg.advanced.bridge_dir.as_ref(),
    );
    set_optional_path(
        &mut doc["advanced"],
        "catalog_path",
        cfg.advanced.catalog_path.as_ref(),
    );
    set_optional_path(
        &mut doc["advanced"],
        "daemon_bin",
        cfg.advanced.daemon_bin.as_ref(),
    );

    set_optional_path(
        &mut doc["official_tools"],
        "td_exe",
        cfg.official_tools.td_exe.as_ref(),
    );
    set_optional_path(
        &mut doc["official_tools"],
        "expand_path",
        cfg.official_tools.expand_path.as_ref(),
    );
    set_optional_path(
        &mut doc["official_tools"],
        "collapse_path",
        cfg.official_tools.collapse_path.as_ref(),
    );
    set_optional_str(
        &mut doc["official_tools"],
        "wine_exe",
        cfg.official_tools.wine_exe.as_deref(),
    );
    set_optional_path(
        &mut doc["official_tools"],
        "wine_prefix",
        cfg.official_tools.wine_prefix.as_ref(),
    );

    set_optional_path(
        &mut doc["project"],
        "template_path",
        cfg.project.template_path.as_ref(),
    );

    set_optional_path(
        &mut doc["palette"],
        "user_root",
        cfg.palette.user_root.as_ref(),
    );
    set_optional_path(
        &mut doc["palette"],
        "store_dir",
        cfg.palette.store_dir.as_ref(),
    );
    if let Some(table) = doc["palette"].as_table_mut() {
        let mut arr = toml_edit::Array::new();
        for pat in &cfg.palette.ignore {
            arr.push(pat.as_str());
        }
        table.insert("ignore", value(arr));
    }

    fs::write(path, doc.to_string()).with_context(|| format!("write config {}", path.display()))?;
    tracing::debug!(path = %path.display(), "config section values applied and saved");
    Ok(())
}

fn ensure_table(doc: &mut DocumentMut, name: &str) {
    if !doc.contains_key(name) || !doc[name].is_table() {
        doc[name] = Item::Table(Table::new());
    }
}

fn set_optional_path(table_item: &mut Item, key: &str, path: Option<&PathBuf>) {
    let Some(table) = table_item.as_table_mut() else {
        return;
    };
    match path {
        Some(p) => {
            table.insert(key, value(p.to_string_lossy().as_ref()));
        }
        None => {
            table.remove(key);
        }
    }
}

fn set_optional_str(table_item: &mut Item, key: &str, val: Option<&str>) {
    let Some(table) = table_item.as_table_mut() else {
        return;
    };
    match val {
        Some(v) => {
            table.insert(key, value(v));
        }
        None => {
            table.remove(key);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn ensure_default_create_then_noop() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        assert!(ensure_default(&path, false).expect("create"));
        assert!(path.is_file());
        let first = fs::read_to_string(&path).expect("read");
        assert!(!ensure_default(&path, false).expect("noop"));
        assert_eq!(fs::read_to_string(&path).expect("read2"), first);
    }

    #[test]
    fn ensure_default_force_overwrites() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "broken = true\n").expect("write");
        assert!(ensure_default(&path, true).expect("force"));
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.contains("keep_alive"));
        assert!(!text.contains("broken"));
    }

    #[test]
    fn load_missing_returns_defaults() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("missing.toml");
        let cfg = load(&path).expect("load");
        assert_eq!(cfg, ConfigFile::default());
    }

    #[test]
    fn load_parses_all_sections() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        ensure_default(&path, true).expect("write");
        let mut text = fs::read_to_string(&path).expect("read");
        text = text.replace("port = 9860", "port = 1234");
        text = text.replace("keep_alive = true", "keep_alive = false");
        text = text.replace("always_on = false", "always_on = true");
        text = text.replace("call_timeout_secs = 45", "call_timeout_secs = 60");
        text = text.replace("script_timeout_secs = 120", "script_timeout_secs = 180");
        fs::write(&path, text).expect("rewrite");
        let cfg = load(&path).expect("load");
        assert_eq!(cfg.server.port, 1234);
        assert!(!cfg.daemon.keep_alive);
        assert!(cfg.daemon.always_on);
        assert!(cfg.daemon.show_tray);
        assert_eq!(cfg.bridge.call_timeout_secs, 60);
        assert_eq!(cfg.bridge.script_timeout_secs, 180);
        assert_eq!(cfg.bridge.heartbeat_interval_secs, 5);
        assert_eq!(cfg.bridge.pong_timeout_secs, 8);
        assert_eq!(cfg.bridge.idle_dead_secs, 20);
    }

    #[test]
    fn load_missing_bridge_section_uses_defaults() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[server]
port = 9860
[daemon]
keep_alive = false
always_on = false
show_tray = true
"#,
        )
        .expect("write");
        let cfg = load(&path).expect("load");
        assert_eq!(cfg.bridge, BridgeSection::default());
    }

    #[test]
    fn bridge_endpoint_round_trips() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        ensure_default(&path, true).expect("seed");
        let mut cfg = load(&path).expect("load");
        assert_eq!(cfg.bridge.host, "127.0.0.1");
        assert_eq!(cfg.bridge.port, DEFAULT_BRIDGE_PORT);
        cfg.bridge.host = "::1".to_owned();
        cfg.bridge.port = 9999;
        save(&path, &cfg).expect("save");
        let again = load(&path).expect("reload");
        assert_eq!(again.bridge.host, "::1");
        assert_eq!(again.bridge.port, 9999);
    }

    #[test]
    fn save_round_trips_and_preserves_comment() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        ensure_default(&path, true).expect("seed");
        let mut cfg = load(&path).expect("load");
        cfg.server.port = 5555;
        cfg.daemon.keep_alive = true;
        cfg.bridge.call_timeout_secs = 90;
        save(&path, &cfg).expect("save");
        let text = fs::read_to_string(&path).expect("read");
        assert!(
            text.contains("td-mcp-rs configuration")
                || text.contains("Keep alive")
                || text.contains("keep_alive")
        );
        assert!(text.contains("5555"));
        assert!(text.contains("call_timeout_secs"));
        let again = load(&path).expect("reload");
        assert_eq!(again.server.port, 5555);
        assert!(again.daemon.keep_alive);
        assert_eq!(again.bridge.call_timeout_secs, 90);
    }

    #[test]
    fn default_toml_parses() {
        let cfg: ConfigFile = toml_edit::de::from_str(DEFAULT_TOML).expect("parse default");
        assert_eq!(cfg.server.port, 9860);
        assert_eq!(cfg.server.bind_address, "127.0.0.1");
        assert_eq!(cfg.auth, AuthSection::default());
        assert!(cfg.daemon.keep_alive);
        assert!(!cfg.daemon.always_on);
        assert!(cfg.daemon.show_tray);
        assert_eq!(cfg.bridge, BridgeSection::default());
    }

    #[test]
    fn auth_and_bind_defaults() {
        let cfg = ConfigFile::default();
        assert_eq!(cfg.server.bind_address, "127.0.0.1");
        assert_eq!(cfg.auth.mode, "none");
        assert!(cfg.auth.psk.is_empty());
    }

    #[test]
    fn missing_auth_section_uses_defaults() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[server]
port = 9860
bind_address = "127.0.0.1"
[daemon]
keep_alive = false
always_on = false
show_tray = true
"#,
        )
        .expect("write");
        let cfg = load(&path).expect("load");
        assert_eq!(cfg.auth, AuthSection::default());
        assert_eq!(cfg.server.bind_address, "127.0.0.1");
    }

    #[test]
    fn save_round_trips_bind_and_auth() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        ensure_default(&path, true).expect("seed");
        let mut cfg = load(&path).expect("load");
        cfg.server.bind_address = "0.0.0.0".to_owned();
        cfg.auth.mode = "psk".to_owned();
        cfg.auth.psk = "secret-token".to_owned();
        save(&path, &cfg).expect("save");
        let again = load(&path).expect("reload");
        assert_eq!(again.server.bind_address, "0.0.0.0");
        assert_eq!(again.auth.mode, "psk");
        assert_eq!(again.auth.psk, "secret-token");
    }

    #[test]
    fn validate_remote_auth_allows_non_loopback_without_psk() {
        // PSK is optional even on a LAN/remote bind — no forced friction.
        let mut cfg = ConfigFile::default();
        cfg.server.bind_address = "0.0.0.0".to_owned();
        assert!(validate_remote_auth(&cfg).is_ok());
        cfg.auth.mode = "psk".to_owned();
        cfg.auth.psk = "tok".to_owned();
        assert!(validate_remote_auth(&cfg).is_ok());
    }

    #[test]
    fn validate_remote_auth_rejects_psk_mode_without_psk() {
        // Explicitly choosing psk mode with an empty secret is just broken,
        // regardless of bind_address.
        let mut cfg = ConfigFile::default();
        cfg.auth.mode = "psk".to_owned();
        assert!(validate_remote_auth(&cfg).is_err());
        let mut remote = cfg.clone();
        remote.server.bind_address = "0.0.0.0".to_owned();
        assert!(validate_remote_auth(&remote).is_err());
    }

    #[test]
    fn validate_remote_auth_accepts_loopback_without_psk() {
        let cfg = ConfigFile::default();
        assert!(validate_remote_auth(&cfg).is_ok());
        let mut v6 = ConfigFile::default();
        v6.server.bind_address = "::1".to_owned();
        assert!(validate_remote_auth(&v6).is_ok());
    }

    #[test]
    fn missing_federation_section_uses_defaults() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[server]
port = 9860
bind_address = "127.0.0.1"
[daemon]
keep_alive = false
always_on = false
show_tray = true
"#,
        )
        .expect("write");
        let cfg = load(&path).expect("load");
        assert_eq!(cfg.federation, FederationSection::default());
        assert_eq!(cfg.federation.role, "standalone");
        assert!(cfg.federation.daemon_id.is_empty());
    }

    #[test]
    fn ensure_daemon_id_generates_once_and_survives_reload() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        ensure_default(&path, true).expect("seed");
        let mut cfg = load(&path).expect("load");
        assert!(cfg.federation.daemon_id.is_empty());
        assert!(ensure_daemon_id(&path, &mut cfg).expect("ensure"));
        let id = cfg.federation.daemon_id.clone();
        assert!(!id.is_empty());
        assert!(!ensure_daemon_id(&path, &mut cfg).expect("noop"));
        assert_eq!(cfg.federation.daemon_id, id);
        let again = load(&path).expect("reload");
        assert_eq!(again.federation.daemon_id, id);
    }

    #[test]
    fn save_round_trips_federation() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        ensure_default(&path, true).expect("seed");
        let mut cfg = load(&path).expect("load");
        cfg.federation.role = "master".to_owned();
        cfg.federation.daemon_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_owned();
        cfg.federation.master_url = "http://127.0.0.1:9860".to_owned();
        cfg.federation.master_psk = "master-secret".to_owned();
        save(&path, &cfg).expect("save");
        let again = load(&path).expect("reload");
        assert_eq!(again.federation.role, "master");
        assert_eq!(
            again.federation.daemon_id,
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        );
        assert_eq!(again.federation.master_url, "http://127.0.0.1:9860");
        assert_eq!(again.federation.master_psk, "master-secret");
    }

    #[test]
    fn default_toml_has_no_optional_logging_overrides() {
        let cfg: ConfigFile = toml_edit::de::from_str(DEFAULT_TOML).expect("parse default");
        assert_eq!(cfg.logging.dir, None);
        assert_eq!(cfg.logging.filter, None);
        assert_eq!(cfg.logging.console_level, None);
        assert_eq!(cfg.logging.max_files, 14);
        assert_eq!(cfg.logging.retention_days, 30);
    }

    #[test]
    fn save_round_trips_palette() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        ensure_default(&path, true).expect("seed");

        // The shipped template already carries the hostile-component blacklist.
        let cfg = load(&path).expect("load");
        assert!(cfg.palette.user_root.is_none());
        assert!(cfg
            .palette
            .ignore
            .iter()
            .any(|p| p == "builtin:TDAbleton/*"));

        let mut cfg = cfg;
        let root = std::path::PathBuf::from("C:/Users/me/Documents/Derivative/Palette");
        cfg.palette.user_root = Some(root.clone());
        cfg.palette.ignore.push("user:Broken/*".into());
        save(&path, &cfg).expect("save");

        let again = load(&path).expect("reload");
        assert_eq!(again.palette.user_root.as_deref(), Some(root.as_path()));
        assert!(again.palette.ignore.iter().any(|p| p == "user:Broken/*"));
        assert!(
            again.palette.store_dir.is_none(),
            "unset optional is dropped"
        );
    }

    #[test]
    fn save_round_trips_official_tools() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        ensure_default(&path, true).expect("seed");
        let cfg = load(&path).expect("load");
        assert!(
            cfg.official_tools.td_exe.is_none(),
            "default template pins nothing"
        );
        let mut cfg = load(&path).expect("load");
        let exe = std::path::PathBuf::from("C:/Program Files/Derivative/TD/bin/TouchDesigner.exe");
        cfg.official_tools.td_exe = Some(exe.clone());
        cfg.official_tools.expand_path = Some(std::path::PathBuf::from("C:/TD/toeexpand.exe"));
        cfg.official_tools.wine_exe = Some("wine64".into());
        cfg.official_tools.wine_prefix = Some(std::path::PathBuf::from("/home/me/.wine"));
        save(&path, &cfg).expect("save");
        let again = load(&path).expect("reload");
        assert_eq!(again.official_tools.td_exe.as_deref(), Some(exe.as_path()));
        assert_eq!(
            again.official_tools.expand_path.as_deref(),
            Some(std::path::Path::new("C:/TD/toeexpand.exe"))
        );
        assert_eq!(again.official_tools.wine_exe.as_deref(), Some("wine64"));
        assert_eq!(
            again.official_tools.wine_prefix.as_deref(),
            Some(std::path::Path::new("/home/me/.wine"))
        );
        // Unset field is dropped on save (comment-preserving optional pattern).
        assert!(again.official_tools.collapse_path.is_none());
    }

    #[test]
    fn missing_logging_section_uses_defaults() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            r#"
[server]
port = 9860
"#,
        )
        .expect("write");
        let cfg = load(&path).expect("load");
        assert_eq!(cfg.logging, LoggingSection::default());
    }

    #[test]
    fn save_round_trips_logging() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        ensure_default(&path, true).expect("seed");
        let mut cfg = load(&path).expect("load");
        cfg.logging.dir = Some(dir.path().join("custom-logs"));
        cfg.logging.filter = Some("debug,hyper=warn".to_owned());
        cfg.logging.console_level = Some("warn".to_owned());
        cfg.logging.max_files = 7;
        cfg.logging.retention_days = 5;
        save(&path, &cfg).expect("save");
        let again = load(&path).expect("reload");
        assert_eq!(again.logging.dir, Some(dir.path().join("custom-logs")));
        assert_eq!(again.logging.filter.as_deref(), Some("debug,hyper=warn"));
        assert_eq!(again.logging.console_level.as_deref(), Some("warn"));
        assert_eq!(again.logging.max_files, 7);
        assert_eq!(again.logging.retention_days, 5);

        // Clearing the optionals removes the keys instead of writing empties.
        let mut cleared = again;
        cleared.logging.dir = None;
        cleared.logging.filter = None;
        cleared.logging.console_level = None;
        save(&path, &cleared).expect("save2");
        let text = fs::read_to_string(&path).expect("read");
        // Keys must be gone; template comments mentioning them may stay.
        for key in ["dir", "filter", "console_level"] {
            assert!(
                !text
                    .lines()
                    .any(|l| l.trim_start().starts_with(&format!("{key} ="))),
                "{key} line must be removed"
            );
        }
        let final_cfg = load(&path).expect("reload2");
        assert_eq!(final_cfg.logging.dir, None);
        assert_eq!(final_cfg.logging.filter, None);
        assert_eq!(final_cfg.logging.console_level, None);
        // Scalars persist across the clear.
        assert_eq!(final_cfg.logging.max_files, 7);
        assert_eq!(final_cfg.logging.retention_days, 5);
    }
}
