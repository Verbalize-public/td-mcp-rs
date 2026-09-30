//! Interactive-session probes used to resolve the effective daemon mode.
//!
//! The GUI is the default, but a tray needs a desktop session to live in.
//! `ensure_daemon` restarts a healthy daemon whenever the caller wants a tray
//! but the live daemon reports itself headless, so a daemon that quietly
//! degraded to headless must be able to *stay* headless without every IDE
//! reconnect bouncing it. That decision is made from the caller's own session
//! (this module) and reported honestly on `/admin/status`, never inferred from
//! a failed window (L-10).

/// Whether this process looks like it can open a desktop window.
///
/// Best-effort preflight, not a guarantee: on Linux the authoritative answer
/// is whether the event loop actually came up, which only the running GUI can
/// report. A desktop can also appear *after* this call (a daemon started from
/// a service unit before the session exists), so this decides the initial mode,
/// never forces one.
pub fn display_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        // Wayland first: a native Wayland session has DISPLAY unset, and
        // XWayland sessions have DISPLAY set. Empty values mean "unset" in
        // practice, so treat them as absent.
        std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
            || std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
    }
    #[cfg(not(target_os = "linux"))]
    {
        // macOS and Windows always have a window server the user can reach;
        // the daemon starts per-user, not as a system service.
        true
    }
}

/// Resolve the headless mode a `start` should actually serve in.
///
/// `requested_headless` is the user's explicit wish (CLI `--no-gui`,
/// `TDMCP_NO_GUI`, or `[daemon].show_tray = false`). A GUI-less *build* has no
/// tray to open at all. Beyond that, this build probes the session, so a
/// headless host settles on headless instead of looping: see [`display_available`].
///
/// Callers use the result as the daemon's reported `noGui` mode, which is what
/// keeps `ensure_daemon` from tearing down and respawning "with tray" a daemon
/// that could never show one.
pub fn effective_headless(requested_headless: bool) -> bool {
    requested_headless || !display_available()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// Env mutation is process-global; serialize the display-variable tests.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Force a known display environment for one assertion on Linux.
    #[cfg(target_os = "linux")]
    fn with_display<R>(wayland: Option<&str>, x11: Option<&str>, body: impl FnOnce() -> R) -> R {
        let restore_wayland = std::env::var_os("WAYLAND_DISPLAY");
        let restore_display = std::env::var_os("DISPLAY");
        set_env("WAYLAND_DISPLAY", wayland);
        set_env("DISPLAY", x11);
        let out = body();
        restore_env("WAYLAND_DISPLAY", restore_wayland);
        restore_env("DISPLAY", restore_display);
        out
    }

    #[cfg(target_os = "linux")]
    fn set_env(key: &str, value: Option<&str>) {
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        };
    }

    #[cfg(target_os = "linux")]
    fn restore_env(key: &str, original: Option<std::ffi::OsString>) {
        match original {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        };
    }

    /// The whole point of the change: the GUI is the default, so the common
    /// case must NOT resolve to headless just because the caller's environment
    /// happened to look bare.
    #[test]
    fn gui_is_the_default() {
        let _guard = env_guard();
        #[cfg(target_os = "linux")]
        with_display(Some("wayland-0"), Some(":0"), || {
            assert!(!effective_headless(false), "gui must be the default");
        });
        #[cfg(not(target_os = "linux"))]
        assert!(!effective_headless(false), "gui must be the default");
    }

    /// An explicit opt-out is always honoured, whatever the session looks like.
    #[test]
    fn explicit_headless_is_always_honoured() {
        let _guard = env_guard();
        assert!(effective_headless(true));
    }

    /// No display session at all (headless box, service account, SSH shell):
    /// settle on headless, and do not claim a tray.
    #[cfg(target_os = "linux")]
    #[test]
    fn no_display_session_resolves_to_headless() {
        let _guard = env_guard();
        with_display(None, None, || {
            assert!(!display_available());
            assert!(effective_headless(false));
        });
    }

    /// An *empty* display variable means "unset" in practice — a shell that
    /// exports `DISPLAY=` is a headless box, not a session.
    #[cfg(target_os = "linux")]
    #[test]
    fn empty_display_variables_count_as_absent() {
        let _guard = env_guard();
        with_display(Some(""), Some(""), || {
            assert!(!display_available());
            assert!(effective_headless(false));
        });
    }

    /// Either protocol alone is a usable desktop session.
    #[cfg(target_os = "linux")]
    #[test]
    fn either_display_protocol_is_enough() {
        let _guard = env_guard();
        with_display(Some("wayland-0"), None, || assert!(display_available()));
        with_display(None, Some(":0"), || assert!(display_available()));
    }
}
