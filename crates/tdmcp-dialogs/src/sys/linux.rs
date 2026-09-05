//! Linux backend: /proc-based process ops only. No window introspection —
//! there is no windowing surface reachable from Rust under Wine here, so
//! dialogs stay `tdmcp.dialog.unsupported` on this platform
//! (docs/LINUX_SUPPORT.md L-8).

use std::fs;

use libc::{c_int, kill};

use super::{SysControl, SysWindow};

/// No windows on Linux without a windowing backend.
pub fn top_level_windows(_pid: u32) -> std::io::Result<Vec<SysWindow>> {
    Ok(Vec::new())
}

/// No controls without a backend.
pub fn child_controls(_id: &str) -> Vec<SysControl> {
    Vec::new()
}

/// No-op click.
pub fn post_click(_id: &str, _ctrl_id: i32) -> bool {
    false
}

/// No-op close.
pub fn post_close(_id: &str) -> bool {
    false
}

/// Never hung without probes.
pub fn is_hung(_id: &str, _budget_ms: u32) -> bool {
    false
}

/// Image basename of `pid` via `/proc/<pid>/exe` (the real ELF target — for a
/// Wine-hosted TD this is `wine`/`wine64`/`wine-preloader`, not
/// `TouchDesigner.exe`; see `lifecycle.rs::is_touchdesigner_image`'s note —
/// this only matters for *unregistered* pids, and a `spawn_td`-launched pid
/// is always registered).
pub fn process_image_name(pid: u32) -> Option<String> {
    let link = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let name = link.file_name()?.to_str()?;
    Some(name.trim_end_matches(" (deleted)").to_string())
}

/// True when `/proc/<pid>` exists and its stat state char is not `Z`
/// (zombie). Mirrors `tdmcp-daemon::ensure::pid_alive`'s Linux arm exactly
/// (duplicated: `tdmcp-dialogs` and `tdmcp-daemon` share no crate).
pub fn process_alive(pid: u32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        // Unreadable (hidepid, exit race) -> fail open: never mis-report a
        // live TD as gone.
        Err(_) => true,
        Ok(text) => !matches!(proc_stat_state(&text), Some('Z')),
    }
}

/// State char from `/proc/<pid>/stat` (`pid (comm) state ppid …`). `comm` may
/// contain spaces/parens, so parse after the last `)`.
fn proc_stat_state(text: &str) -> Option<char> {
    let after_comm = text.rsplit_once(')')?.1;
    after_comm.split_whitespace().next()?.chars().next()
}

/// No windows exist on Linux without a windowing backend, so graceful close
/// always falls straight to SIGTERM (matches L-5's literal spec).
pub fn close_pid_windows(pid: u32) -> usize {
    let Ok(pid_i) = c_int::try_from(pid) else {
        return 0;
    };
    // SAFETY: SIGTERM is the standard graceful terminate signal to a plain pid.
    usize::from(unsafe { kill(pid_i, libc::SIGTERM) == 0 })
}

/// Hard-terminate via SIGKILL.
pub fn terminate_process(pid: u32) -> bool {
    let Ok(pid_i) = c_int::try_from(pid) else {
        return false;
    };
    // SAFETY: SIGKILL is the Unix last-resort terminate.
    unsafe { kill(pid_i, libc::SIGKILL) == 0 }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn process_alive_current_pid() {
        assert!(process_alive(std::process::id()));
    }

    #[test]
    fn process_alive_dead_pid() {
        assert!(!process_alive(999_999_999));
    }

    #[test]
    fn process_image_name_current_pid_matches_current_exe() {
        let exe = std::env::current_exe().expect("current_exe");
        let want = exe.file_name().and_then(|n| n.to_str()).map(str::to_string);
        assert_eq!(process_image_name(std::process::id()), want);
    }

    /// Regression mirror of the macOS test: an exited-but-unreaped child must
    /// read as dead, not alive.
    #[test]
    fn process_alive_false_for_unreaped_zombie() {
        let mut child = std::process::Command::new("/bin/true")
            .spawn()
            .expect("spawn /bin/true");
        let pid = child.id();
        let mut zombie = false;
        for _ in 0..200 {
            if matches!(
                fs::read_to_string(format!("/proc/{pid}/stat"))
                    .ok()
                    .as_deref()
                    .and_then(proc_stat_state),
                Some('Z')
            ) {
                zombie = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(zombie, "child never became an unreaped zombie");
        assert!(!process_alive(pid), "zombie must not count as alive");
        let _ = child.wait(); // reap
    }
}
