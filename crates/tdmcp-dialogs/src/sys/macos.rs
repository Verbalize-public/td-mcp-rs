//! macOS backend placeholder (P2): returns unsupported / empty data today.
//! Real implementation sketches CGWindowList + Accessibility API per
//! DIALOGS.md §7 sketch; the facade surface stays identical.

use super::{SysControl, SysWindow};

/// No enumeration yet — always empty.
pub fn top_level_windows(_pid: u32) -> std::io::Result<Vec<SysWindow>> {
    Ok(Vec::new())
}

/// No content extraction yet.
pub fn child_controls(_id: &str) -> Vec<SysControl> {
    Vec::new()
}

/// Never clicks yet.
pub fn post_click(_id: &str, _ctrl_id: i32) -> bool {
    false
}

/// Never closes yet.
pub fn post_close(_id: &str) -> bool {
    false
}

/// Unknown responsiveness without a backend.
pub fn is_hung(_id: &str, _budget_ms: u32) -> bool {
    false
}

/// Unknown image name without a backend.
pub fn process_image_name(_pid: u32) -> Option<String> {
    None
}
