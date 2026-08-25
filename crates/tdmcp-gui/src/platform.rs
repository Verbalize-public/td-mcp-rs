//! OS-integration shims: notifications and file-manager reveal.
//!
//! Platform notes live with their code: macOS toasts spawn `osascript`
//! so AppKit never re-enters winit's event loop (see `toast` below).

use std::path::Path;
use std::process::Command;

use anyhow::Result;
use tracing::warn;

/// Show an OS toast (best-effort; failures are logged).
///
/// On macOS, `notify-rust` talks to AppKit/`NSUserNotification` and can re-enter
/// the run loop from inside a winit callback, aborting with
/// "tried to handle event while another event is currently being handled".
/// Fire a separate `osascript` process instead so the notification never shares
/// our event loop.
pub fn toast(summary: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let summary = summary.to_owned();
        let body = body.to_owned();
        let spawn = std::thread::Builder::new()
            .name("tdmcp-toast".into())
            .spawn(move || {
                let script = format!(
                    "display notification \"{}\" with title \"{}\"",
                    applescript_escape(&body),
                    applescript_escape(&summary),
                );
                match Command::new("osascript")
                    .args(["-e", &script])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                {
                    Ok(status) if status.success() => {}
                    Ok(status) => warn!(?status, summary, "osascript toast non-zero"),
                    Err(e) => warn!(error = %e, summary, "osascript toast failed"),
                }
            });
        if let Err(e) = spawn {
            warn!(error = %e, "toast thread spawn failed");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        match notify_rust::Notification::new()
            .summary(summary)
            .body(body)
            .appname("td-mcp-rs")
            .show()
        {
            Ok(_) => {}
            Err(e) => warn!(error = %e, summary, "OS toast failed"),
        }
    }
}

#[cfg(target_os = "macos")]
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn notify(summary: &str, body: &str) {
    toast(summary, body);
}

/// Open the file manager on `target` (select file when it is a file; else open dir).
/// `fallback_dir` is used when `target` is missing.
pub(crate) fn reveal_in_file_manager(target: &Path, fallback_dir: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        if target.is_file() {
            Command::new("explorer")
                .arg(format!("/select,{}", target.display()))
                .spawn()
                .map_err(|e| anyhow::anyhow!("explorer /select: {e}"))?;
        } else {
            let dir = if target.is_dir() {
                target
            } else {
                fallback_dir
            };
            Command::new("explorer")
                .arg(dir.as_os_str())
                .spawn()
                .map_err(|e| anyhow::anyhow!("explorer: {e}"))?;
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        if target.is_file() {
            Command::new("open")
                .args(["-R", &target.to_string_lossy()])
                .spawn()
                .map_err(|e| anyhow::anyhow!("open -R: {e}"))?;
        } else {
            let dir = if target.is_dir() {
                target
            } else {
                fallback_dir
            };
            Command::new("open")
                .arg(dir)
                .spawn()
                .map_err(|e| anyhow::anyhow!("open: {e}"))?;
        }
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = if target.is_dir() {
            target
        } else if target.is_file() {
            target.parent().unwrap_or(fallback_dir)
        } else {
            fallback_dir
        };
        Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map_err(|e| anyhow::anyhow!("xdg-open: {e}"))?;
        Ok(())
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (target, fallback_dir);
        anyhow::bail!("reveal not supported on this platform");
    }
}
