//! OS login autostart reconciliation for `daemon.always_on`.

use std::path::Path;

use anyhow::{Context, Result};
use auto_launch::{AutoLaunch, AutoLaunchBuilder};
use tracing::{info, warn};

const APP_NAME: &str = "td-mcp-rs";

/// Idempotently enable or disable OS autostart to match `always_on`.
pub fn reconcile(always_on: bool, exe: &Path) -> Result<()> {
    let auto = build(exe)?;
    let enabled = auto
        .is_enabled()
        .with_context(|| "query OS autostart state")?;
    if always_on {
        // Refresh the executable path too: an enabled entry may still point
        // at an old installer location after an upgrade.
        auto.enable().with_context(|| "enable OS autostart")?;
        info!(exe = %exe.display(), "autostart enabled");
    } else if !always_on && enabled {
        auto.disable().with_context(|| "disable OS autostart")?;
        info!("autostart disabled");
    } else {
        info!(always_on, enabled, "autostart already reconciled");
    }
    Ok(())
}

/// Best-effort reconcile — log and continue on failure (non-fatal).
pub fn reconcile_best_effort(always_on: bool, exe: &Path) {
    if let Err(e) = reconcile(always_on, exe) {
        warn!(error = %e, always_on, "autostart reconcile failed");
    }
}

fn build(exe: &Path) -> Result<AutoLaunch> {
    let path = exe
        .to_str()
        .with_context(|| format!("autostart exe path not utf-8: {}", exe.display()))?
        .to_owned();
    let mut builder = AutoLaunchBuilder::new();
    builder
        .set_app_name(APP_NAME)
        .set_app_path(&path)
        .set_args(&["start"]);
    #[cfg(windows)]
    {
        builder.set_windows_enable_mode(auto_launch::WindowsEnableMode::CurrentUser);
    }
    builder.build().context("build AutoLaunch")
}
