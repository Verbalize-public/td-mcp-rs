//! macOS backend: CGWindowList enumeration + Accessibility API for content/actions.
//!
//! Requires TCC Accessibility permission for AX calls; window listing works
//! without it. Fail-open on permission errors (empty popups, never crash).

#![allow(clippy::undocumented_unsafe_blocks)]

use std::path::Path;
use std::ptr::NonNull;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::window::{
    kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
    CGWindowListCopyWindowInfo,
};
use libc::{c_int, kill, sysctl, KERN_PROC};
use objc2_application_services::{AXError, AXIsProcessTrusted, AXUIElement};
use objc2_core_foundation::{CFArray as ObjCFArray, CFRetained, CFString as ObjCFString, CFType as ObjCFType, Type};

use super::{SysControl, SysWindow};

const KERN_PROC_PIDPATH: c_int = 12;
const AX_ATTR_ROLE: &str = "AXRole";
const AX_ATTR_SUBROLE: &str = "AXSubrole";
const AX_ATTR_TITLE: &str = "AXTitle";
const AX_ATTR_VALUE: &str = "AXValue";
const AX_ATTR_CHILDREN: &str = "AXChildren";
const AX_ATTR_WINDOWS: &str = "AXWindows";
const AX_ACTION_PRESS: &str = "AXPress";
const AX_ROLE_BUTTON: &str = "AXButton";

fn cf_dict_get_string(dict: &CFDictionary<CFString, CFType>, key: &str) -> Option<String> {
    let key_cf = CFString::new(key);
    let value = dict.find(&key_cf)?;
    if let Some(s) = value.downcast::<CFString>() {
        return Some(s.to_string());
    }
    None
}

fn cf_dict_get_number(dict: &CFDictionary<CFString, CFType>, key: &str) -> Option<i64> {
    let key_cf = CFString::new(key);
    let value = dict.find(&key_cf)?;
    if let Some(n) = value.downcast::<CFNumber>() {
        return n.to_i64();
    }
    None
}

fn cf_dict_get_bool(dict: &CFDictionary<CFString, CFType>, key: &str) -> bool {
    cf_dict_get_number(dict, key).is_some_and(|n| n != 0)
}

fn cg_window_list() -> CFArray<CFDictionary<CFString, CFType>> {
    // SAFETY: CGWindowListCopyWindowInfo returns an owned CFArray.
    let list = unsafe {
        CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        )
    };
    if list.is_null() {
        return CFArray::from_CFTypes(&[] as &[CFDictionary<CFString, CFType>]);
    }
    unsafe { CFArray::wrap_under_create_rule(list) }
}

fn lookup_cg_window(window_number: &str) -> Option<(u32, String)> {
    let target = window_number.parse::<i64>().ok()?;
    let array = cg_window_list();
    for i in 0..array.len() {
        let dict = array.get(i)?;
        let num = cf_dict_get_number(&dict, "kCGWindowNumber")?;
        if num != target {
            continue;
        }
        let pid = cf_dict_get_number(&dict, "kCGWindowOwnerPID")? as u32;
        let title = cf_dict_get_string(&dict, "kCGWindowName").unwrap_or_default();
        return Some((pid, title));
    }
    None
}

/// Enumerate on-screen windows owned by `pid` via CGWindowList.
pub fn top_level_windows(pid: u32) -> std::io::Result<Vec<SysWindow>> {
    let array = cg_window_list();
    let mut out = Vec::new();
    for i in 0..array.len() {
        let Some(dict) = array.get(i) else {
            continue;
        };
        let owner_pid = cf_dict_get_number(&dict, "kCGWindowOwnerPID").unwrap_or(-1);
        if owner_pid != i64::from(pid) {
            continue;
        }
        let layer = cf_dict_get_number(&dict, "kCGWindowLayer").unwrap_or(0);
        if layer < 0 {
            continue;
        }
        let id = cf_dict_get_number(&dict, "kCGWindowNumber")
            .map(|n| n.to_string())
            .unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let title = cf_dict_get_string(&dict, "kCGWindowName").unwrap_or_default();
        let owner = cf_dict_get_string(&dict, "kCGWindowOwnerName").unwrap_or_default();
        let visible = cf_dict_get_bool(&dict, "kCGWindowIsOnscreen");
        out.push(SysWindow {
            pid,
            id,
            class: owner,
            title,
            visible,
            styles: layer as isize,
            ex_styles: 0,
        });
    }
    Ok(out)
}

/// Whether TCC granted Accessibility for this process (AX automation).
pub fn accessibility_trusted() -> bool {
    // SAFETY: AXIsProcessTrusted has no preconditions.
    unsafe { AXIsProcessTrusted() }
}

fn ax_trusted() -> bool {
    accessibility_trusted()
}

fn ax_copy_value(el: &AXUIElement, attr: &str) -> Option<CFRetained<ObjCFType>> {
    let attr_cf = ObjCFString::from_str(attr);
    let mut value: *const ObjCFType = std::ptr::null();
    // SAFETY: `value` is a valid out-pointer for CopyAttributeValue.
    let err = unsafe {
        el.copy_attribute_value(
            &attr_cf,
            NonNull::from(&mut value),
        )
    };
    if err != AXError::Success || value.is_null() {
        return None;
    }
    // SAFETY: AX returns a +1 CF object on success.
    let ptr = NonNull::new(value.cast_mut())?;
    // SAFETY: retain valid CFType pointer returned by AX API.
    Some(unsafe { CFRetained::retain(ptr) })
}

fn ax_attr_string(el: &AXUIElement, attr: &str) -> Option<String> {
    let value = ax_copy_value(el, attr)?;
    value
        .downcast::<ObjCFString>()
        .ok()
        .map(|s| s.to_string())
}

fn ax_children(el: &AXUIElement) -> Vec<CFRetained<AXUIElement>> {
    let Some(value) = ax_copy_value(el, AX_ATTR_CHILDREN) else {
        return Vec::new();
    };
    let Ok(arr) = value.downcast::<ObjCFArray>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..arr.count() {
        // SAFETY: AXChildren array holds AXUIElement references.
        let ptr = unsafe { arr.value_at_index(i) };
        if ptr.is_null() {
            continue;
        }
        let child: &AXUIElement = unsafe { &*(ptr.cast()) };
        out.push(child.retain());
    }
    out
}

fn ax_window_for_id(window_number: &str) -> Option<CFRetained<AXUIElement>> {
    if !ax_trusted() {
        return None;
    }
    let (pid, title) = lookup_cg_window(window_number)?;
    // SAFETY: pid is a live process id from CGWindowList.
    let app = unsafe { AXUIElement::new_application(pid as i32) };
    let Some(value) = ax_copy_value(&app, AX_ATTR_WINDOWS) else {
        return None;
    };
    let Ok(arr) = value.downcast::<ObjCFArray>() else {
        return None;
    };
    for i in 0..arr.count() {
        // SAFETY: AXWindows array holds AXUIElement references.
        let ptr = unsafe { arr.value_at_index(i) };
        if ptr.is_null() {
            continue;
        }
        let win: &AXUIElement = unsafe { &*(ptr.cast()) };
        let win_title = ax_attr_string(win, AX_ATTR_TITLE).unwrap_or_default();
        if !title.is_empty() && win_title == title {
            return Some(win.retain());
        }
    }
    for i in (0..arr.count()).rev() {
        let ptr = unsafe { arr.value_at_index(i) };
        if ptr.is_null() {
            continue;
        }
        let win: &AXUIElement = unsafe { &*(ptr.cast()) };
        let win_title = ax_attr_string(win, AX_ATTR_TITLE).unwrap_or_default();
        if !win_title.is_empty() && !win_title.to_lowercase().starts_with("touchdesigner") {
            return Some(win.retain());
        }
    }
    let ptr = unsafe { arr.value_at_index(0) };
    if ptr.is_null() {
        return None;
    }
    let win: &AXUIElement = unsafe { &*(ptr.cast()) };
    Some(win.retain())
}

fn walk_ax_controls(el: &AXUIElement, out: &mut Vec<SysControl>, depth: u32) {
    if depth > 12 {
        return;
    }
    let role = ax_attr_string(el, AX_ATTR_ROLE).unwrap_or_default();
    let subrole = ax_attr_string(el, AX_ATTR_SUBROLE).unwrap_or_default();
    let label = ax_attr_string(el, AX_ATTR_TITLE)
        .or_else(|| ax_attr_string(el, AX_ATTR_VALUE))
        .unwrap_or_default();
    let is_button = role == AX_ROLE_BUTTON || subrole.contains("Button");
    if is_button || role == "AXStaticText" || role == "AXText" {
        let ctrl_id = out.len() as i32 + 1;
        let is_default = subrole.contains("Default");
        out.push(SysControl {
            id: format!("ax-{ctrl_id}"),
            class: role,
            label,
            ctrl_id: Some(ctrl_id),
            is_default,
        });
    }
    for child in ax_children(el) {
        walk_ax_controls(&child, out, depth + 1);
    }
}

/// Accessibility tree controls for one window id (CGWindowNumber).
pub fn child_controls(id: &str) -> Vec<SysControl> {
    let Some(win) = ax_window_for_id(id) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk_ax_controls(&win, &mut out, 0);
    out
}

fn ax_press(el: &AXUIElement) -> bool {
    let action = ObjCFString::from_str(AX_ACTION_PRESS);
    // SAFETY: AXPress on a valid element.
    unsafe { el.perform_action(&action) == AXError::Success }
}

fn find_and_press(el: &AXUIElement, target_label: &str, depth: u32) -> bool {
    if depth > 12 {
        return false;
    }
    let label = ax_attr_string(el, AX_ATTR_TITLE)
        .or_else(|| ax_attr_string(el, AX_ATTR_VALUE))
        .unwrap_or_default();
    if label == target_label {
        return ax_press(el);
    }
    for child in ax_children(el) {
        if find_and_press(&child, target_label, depth + 1) {
            return true;
        }
    }
    false
}

/// Click a control by ctrl_id within the window's AX tree.
pub fn post_click(id: &str, ctrl_id: i32) -> bool {
    let controls = child_controls(id);
    let Some(ctrl) = controls.iter().find(|c| c.ctrl_id == Some(ctrl_id)) else {
        return false;
    };
    let Some(win) = ax_window_for_id(id) else {
        return false;
    };
    find_and_press(&win, &ctrl.label, 0)
}

/// Close/minimize via AX close button or cancel action.
pub fn post_close(id: &str) -> bool {
    let controls = child_controls(id);
    if let Some(ctrl) = controls.iter().find(|c| {
        c.label.eq_ignore_ascii_case("close")
            || c.label.eq_ignore_ascii_case("cancel")
            || c.label.eq_ignore_ascii_case("ok")
    }) {
        if let Some(cid) = ctrl.ctrl_id {
            return post_click(id, cid);
        }
    }
    false
}

/// Best-effort hung detection — AX calls can block on wedged apps.
pub fn is_hung(_id: &str, _budget_ms: u32) -> bool {
    false
}

/// Image basename of `pid`, e.g. `"TouchDesigner"`.
pub fn process_image_name(pid: u32) -> Option<String> {
    let mut mib: [c_int; 4] = [libc::CTL_KERN, KERN_PROC, KERN_PROC_PIDPATH, pid as c_int];
    let mut buf = [0u8; 4096];
    let mut size = buf.len();
    // SAFETY: sysctl with valid mib and buffer.
    let rc = unsafe {
        sysctl(
            mib.as_mut_ptr(),
            4,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || size == 0 {
        return None;
    }
    let path = std::str::from_utf8(&buf[..size.saturating_sub(1)]).ok()?;
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
}

/// Cheap liveness probe via signal 0.
pub fn process_alive(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) does not send a signal; checks existence.
    unsafe { kill(pid as i32, 0) == 0 }
}

/// Post close to every visible top-level window of `pid` (graceful kill helper).
pub fn close_pid_windows(pid: u32) -> usize {
    let Ok(windows) = top_level_windows(pid) else {
        return 0;
    };
    let mut sent = 0usize;
    for w in windows {
        if w.visible && post_close(&w.id) {
            sent += 1;
        }
    }
    if sent == 0 {
        // Fallback: SIGTERM when no AX close succeeded.
        // SAFETY: graceful terminate signal.
        if unsafe { kill(pid as i32, libc::SIGTERM) == 0 } {
            sent = 1;
        }
    }
    sent
}

/// Hard-terminate via SIGKILL.
pub fn terminate_process(pid: u32) -> bool {
    // SAFETY: SIGKILL is the Unix last-resort terminate.
    unsafe { kill(pid as i32, libc::SIGKILL) == 0 }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "unit tests")]
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
}
