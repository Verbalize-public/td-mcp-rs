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
        // Same thread hand-off as the macOS path: notify-rust's show() is a
        // synchronous DBus/WinRT round-trip and must never block the caller
        // (the idle watcher fires toasts from inside an async task).
        let summary = summary.to_owned();
        let body = body.to_owned();
        let spawn = std::thread::Builder::new()
            .name("tdmcp-toast".into())
            .spawn(move || {
                match notify_rust::Notification::new()
                    .summary(&summary)
                    .body(&body)
                    .appname("td-mcp-rs")
                    .show()
                {
                    Ok(_) => {}
                    Err(e) => warn!(error = %e, summary, "OS toast failed"),
                }
            });
        if let Err(e) = spawn {
            warn!(error = %e, "toast thread spawn failed");
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

/// Glance-popup close-on-outside-click (X11 only).
///
/// Under focus-follows-mouse window managers (Hyprland default) the popup
/// loses focus whenever the cursor merely moves off it, so "hide on focus
/// loss" cannot tell an outside click from a mouse move — and a click outside
/// our window is never delivered to us at all. X11 pointer state is
/// server-global, so polling `XQueryPointer` on a private connection sees
/// presses anywhere on screen (unsafe enclave: RISKS.md R10). The close rule
/// lives in `DashboardApp::poll_outside_click_close`.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
pub(crate) mod glance_pointer {
    use std::ffi::{c_int, c_uint};
    use std::ptr::NonNull;

    use x11_dl::xlib::{self, Xlib};

    /// Button1|Button2|Button3 — wheel buttons 4/5 excluded so scrolling
    /// elsewhere never closes the popup.
    const CLOSE_BUTTON_MASK: c_uint = xlib::Button1Mask | xlib::Button2Mask | xlib::Button3Mask;

    pub(crate) struct PointerPoller {
        xlib: Xlib,
        display: NonNull<xlib::Display>,
    }

    impl PointerPoller {
        /// Open a private X connection. `None` when Xlib cannot be loaded or
        /// no display is reachable — the caller then keeps the focus-loss
        /// hide fallback.
        pub(crate) fn open() -> Option<Self> {
            let xlib = Xlib::open().ok()?;
            // SAFETY: symbols borrowed from `xlib`, which outlives `display`
            // in this struct; a NULL return (no display) maps to `None`.
            let display = NonNull::new(unsafe { (xlib.XOpenDisplay)(std::ptr::null()) })?;
            Some(Self { xlib, display })
        }

        /// Mask of physical buttons currently held anywhere on screen, or
        /// `None` when the query fails.
        pub(crate) fn held_buttons(&self) -> Option<c_uint> {
            let dpy = self.display.as_ptr();
            // SAFETY: all pointers are local out-params; `dpy` is alive for
            // the lifetime of `self` and only used from the GUI thread.
            unsafe {
                let root = (self.xlib.XDefaultRootWindow)(dpy);
                let (mut r, mut c) = (0 as xlib::Window, 0 as xlib::Window);
                let (mut rx, mut ry, mut wx, mut wy) = (0 as c_int, 0, 0, 0);
                let mut mask: c_uint = 0;
                let ok = (self.xlib.XQueryPointer)(
                    dpy, root, &mut r, &mut c, &mut rx, &mut ry, &mut wx, &mut wy, &mut mask,
                );
                (ok != 0).then_some(mask & CLOSE_BUTTON_MASK)
            }
        }
    }

    impl Drop for PointerPoller {
        fn drop(&mut self) {
            // SAFETY: single close of the connection opened in `open`.
            unsafe { (self.xlib.XCloseDisplay)(self.display.as_ptr()) };
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) mod glance_pointer {
    /// Non-X11 stub: outside-click polling is unavailable and the popup keeps
    /// its hide-on-focus-loss behaviour.
    pub(crate) struct PointerPoller;

    impl PointerPoller {
        pub(crate) fn open() -> Option<Self> {
            None
        }

        pub(crate) fn held_buttons(&self) -> Option<u32> {
            None
        }
    }
}
