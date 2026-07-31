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

/// Full on-disk config document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConfigFile {
	/// Listen / MCP server settings.
	pub server: ServerSection,
	/// Daemon lifecycle settings.
	pub daemon: DaemonSection,
	/// Optional path overrides.
	pub advanced: AdvancedSection,
}

impl Default for ConfigFile {
	fn default() -> Self {
		Self {
			server: ServerSection::default(),
			daemon: DaemonSection::default(),
			advanced: AdvancedSection::default(),
		}
	}
}

/// `[server]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSection {
	/// HTTP listen port (MCP + admin).
	pub port: u16,
}

impl Default for ServerSection {
	fn default() -> Self {
		Self { port: 9860 }
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
			keep_alive: false,
			always_on: false,
			show_tray: true,
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
		key: "server.port",
		label: "Port",
		help: "HTTP listen port for MCP and admin (default 9860).",
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
		.join("tdmcp-rs")
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
		return Ok(ConfigFile::default());
	}
	let text = fs::read_to_string(path)
		.with_context(|| format!("read config {}", path.display()))?;
	let file: ConfigFile = toml_edit::de::from_str(&text)
		.with_context(|| format!("parse config {}", path.display()))?;
	Ok(file)
}

/// Save `cfg` into `path`, preserving comments/formatting when possible.
pub fn save(path: &Path, cfg: &ConfigFile) -> Result<()> {
	if let Some(parent) = path.parent() {
		fs::create_dir_all(parent)
			.with_context(|| format!("create config dir {}", parent.display()))?;
	}

	let existing = if path.is_file() {
		fs::read_to_string(path)
			.with_context(|| format!("read config {}", path.display()))?
	} else {
		DEFAULT_TOML.to_owned()
	};

	let mut doc = existing
		.parse::<DocumentMut>()
		.unwrap_or_else(|_| DocumentMut::new());

	ensure_table(&mut doc, "server");
	ensure_table(&mut doc, "daemon");
	ensure_table(&mut doc, "advanced");

	doc["server"]["port"] = value(i64::from(cfg.server.port));
	doc["daemon"]["keep_alive"] = value(cfg.daemon.keep_alive);
	doc["daemon"]["always_on"] = value(cfg.daemon.always_on);
	doc["daemon"]["show_tray"] = value(cfg.daemon.show_tray);

	set_optional_path(&mut doc["advanced"], "data_dir", cfg.advanced.data_dir.as_ref());
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

	fs::write(path, doc.to_string())
		.with_context(|| format!("write config {}", path.display()))?;
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
		text = text.replace("keep_alive = false", "keep_alive = true");
		text = text.replace("always_on = false", "always_on = true");
		fs::write(&path, text).expect("rewrite");
		let cfg = load(&path).expect("load");
		assert_eq!(cfg.server.port, 1234);
		assert!(cfg.daemon.keep_alive);
		assert!(cfg.daemon.always_on);
		assert!(cfg.daemon.show_tray);
	}

	#[test]
	fn save_round_trips_and_preserves_comment() {
		let dir = tempdir().expect("tempdir");
		let path = dir.path().join("config.toml");
		ensure_default(&path, true).expect("seed");
		let mut cfg = load(&path).expect("load");
		cfg.server.port = 5555;
		cfg.daemon.keep_alive = true;
		save(&path, &cfg).expect("save");
		let text = fs::read_to_string(&path).expect("read");
		assert!(text.contains("td-mcp-rs configuration") || text.contains("Keep alive") || text.contains("keep_alive"));
		assert!(text.contains("5555"));
		let again = load(&path).expect("reload");
		assert_eq!(again.server.port, 5555);
		assert!(again.daemon.keep_alive);
	}

	#[test]
	fn default_toml_parses() {
		let cfg: ConfigFile = toml_edit::de::from_str(DEFAULT_TOML).expect("parse default");
		assert_eq!(cfg.server.port, 9860);
		assert!(!cfg.daemon.keep_alive);
		assert!(!cfg.daemon.always_on);
		assert!(cfg.daemon.show_tray);
	}
}
