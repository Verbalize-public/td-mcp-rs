//! Non-Windows/non-macOS stub: empty snapshots, no platform calls.

use super::{SysControl, SysWindow};

/// No windows on unsupported Unix targets.
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
pub fn is_hung(_id: &str, _budget_ms: u64) -> bool {
    false
}

/// Unknown image name.
pub fn process_image_name(_pid: u32) -> Option<String> {
    None
}

/// Always dead without a probe.
pub fn process_alive(_pid: u32) -> bool {
    false
}

/// No windows to close.
pub fn close_pid_windows(_pid: u32) -> usize {
    let _ = _pid;
    0
}

/// Force kill unsupported.
pub fn terminate_process(_pid: u32) -> bool {
    false
}
