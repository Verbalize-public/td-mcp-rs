//! macOS backend: Accessibility-primary window enumeration + content/actions.
//!
//! AX is the source of truth for windows, titles and dialog classification, so
//! the whole backend needs only the **Accessibility** TCC grant. The earlier
//! CGWindowList-primary design also required **Screen Recording** (that grant
//! alone populates `kCGWindowName`); without it every title came back empty,
//! `classify::is_chrome_title("")` filtered every window, and macOS detected
//! zero popups — silently. CGWindowList survives only as a pid lookup for a
//! window id, which needs no grant at all.
//!
//! AX messaging is bounded ([`AX_MESSAGING_TIMEOUT_SECS`]) because AX calls
//! route through the target app's main thread — the watcher must not hang on
//! the wedged TD it exists to diagnose.

#![allow(clippy::undocumented_unsafe_blocks)]

use std::path::Path;
use std::ptr::NonNull;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::window::{
    kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionAll,
    CGWindowListCopyWindowInfo,
};
use libc::{c_int, kill, sysctl, KERN_PROC};
use objc2_application_services::{AXError, AXIsProcessTrusted, AXUIElement};
use objc2_core_foundation::{
    CFArray as ObjCFArray, CFBoolean, CFRetained, CFString as ObjCFString, CFType as ObjCFType,
    Type,
};

use super::{SysControl, SysWindow};

const KERN_PROC_PIDPATH: c_int = 12;
const AX_ATTR_ROLE: &str = "AXRole";
const AX_ATTR_SUBROLE: &str = "AXSubrole";
const AX_ATTR_TITLE: &str = "AXTitle";
const AX_ATTR_VALUE: &str = "AXValue";
const AX_ATTR_CHILDREN: &str = "AXChildren";
const AX_ATTR_WINDOWS: &str = "AXWindows";
const AX_ATTR_MODAL: &str = "AXModal";
const AX_ATTR_DEFAULT_BUTTON: &str = "AXDefaultButton";
const AX_ATTR_CANCEL_BUTTON: &str = "AXCancelButton";
const AX_ACTION_PRESS: &str = "AXPress";
const AX_ROLE_BUTTON: &str = "AXButton";
const AX_ROLE_SHEET: &str = "AXSheet";
const AX_ROLE_STATIC_TEXT: &str = "AXStaticText";
const AX_ROLE_TEXT: &str = "AXText";
const AX_SUBROLE_DIALOG: &str = "AXDialog";
const AX_SUBROLE_SYSTEM_DIALOG: &str = "AXSystemDialog";
const AX_SUBROLE_STANDARD_WINDOW: &str = "AXStandardWindow";
const AX_SUBROLE_CLOSE_BUTTON: &str = "AXCloseButton";

/// Window-frame widgets. They are `AXButton`s, but they belong to the window
/// chrome, not to the dialog's content, and reporting them as dialog buttons is
/// actively harmful: `describe` would advertise three unlabeled buttons and a
/// dismissal could "press" zoom or minimize instead of answering the dialog.
/// Live-recorded 2026-08-30 against a TouchDesigner `ui.messageBox`, whose whole
/// AX subtree is exactly these three plus one static text.
const AX_CHROME_BUTTON_SUBROLES: [&str; 5] = [
    AX_SUBROLE_CLOSE_BUTTON,
    "AXZoomButton",
    "AXMinimizeButton",
    "AXFullScreenButton",
    "AXToolbarButton",
];

/// Normalized class for a standard (non-dialog) application window.
pub const CLASS_STANDARD_WINDOW: &str = "AXStandardWindow";
/// Normalized class for a dialog-ish window (sheet / dialog / modal).
pub const CLASS_DIALOG: &str = "AXDialog";

/// AX messaging budget. Well under `SNAPSHOT_BUDGET` (150 ms) so a wedged or
/// slow TouchDesigner degrades to "no data" instead of stalling the watcher.
const AX_MESSAGING_TIMEOUT_SECS: f32 = 0.1;

/// Max depth of an AX control subtree walk.
const AX_WALK_DEPTH: u32 = 12;

extern "C" {
    /// Private HIServices export mapping an AX window element to its
    /// `CGWindowID`. Used so popup ids stay identical to the CGWindowList
    /// numbering that shipped previously (and so `describe`/`dismiss` re-resolve
    /// an id to *exactly* the same window rather than by fuzzy title match).
    /// Verified present on macOS 26.1; every call site tolerates failure.
    fn _AXUIElementGetWindow(element: *const AXUIElement, out: *mut u32) -> AXError;
}

// ---------------------------------------------------------------------------
// CoreFoundation helpers (CGWindowList pid lookup only)
// ---------------------------------------------------------------------------

fn cf_dict_get_number(dict: &CFDictionary<CFString, CFType>, key: &str) -> Option<i64> {
    let key_cf = CFString::new(key);
    let value = dict.find(&key_cf)?;
    if let Some(n) = value.downcast::<CFNumber>() {
        return n.to_i64();
    }
    None
}

fn cg_window_list() -> CFArray<CFDictionary<CFString, CFType>> {
    // SAFETY: CGWindowListCopyWindowInfo returns an owned CFArray (or null).
    let list = unsafe {
        CGWindowListCopyWindowInfo(
            kCGWindowListOptionAll | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        )
    };
    if list.is_null() {
        return CFArray::from_CFTypes(&[] as &[CFDictionary<CFString, CFType>]);
    }
    // SAFETY: non-null owned CFArrayRef from a Copy-rule call.
    unsafe { CFArray::wrap_under_create_rule(list) }
}

/// Owning pid of a window id. `kCGWindowOwnerPID` is available without any TCC
/// grant — only `kCGWindowName` requires Screen Recording, and we no longer
/// read it.
fn pid_for_window_id(window_number: &str) -> Option<u32> {
    let target = window_number.parse::<i64>().ok()?;
    let array = cg_window_list();
    for i in 0..array.len() {
        let dict = array.get(i)?;
        if cf_dict_get_number(&dict, "kCGWindowNumber")? != target {
            continue;
        }
        return u32::try_from(cf_dict_get_number(&dict, "kCGWindowOwnerPID")?).ok();
    }
    None
}

// ---------------------------------------------------------------------------
// Accessibility helpers
// ---------------------------------------------------------------------------

/// Whether TCC granted Accessibility for this process (AX automation).
pub fn accessibility_trusted() -> bool {
    // SAFETY: AXIsProcessTrusted has no preconditions.
    unsafe { AXIsProcessTrusted() }
}

/// Application element for `pid` with a bounded messaging timeout.
fn ax_app(pid: u32) -> Option<CFRetained<AXUIElement>> {
    if !accessibility_trusted() {
        return None;
    }
    let pid = i32::try_from(pid).ok()?;
    // SAFETY: pid is a plain process id; AX tolerates dead pids by erroring.
    let app = unsafe { AXUIElement::new_application(pid) };
    // SAFETY: bounds every subsequent call on this element (W2).
    let _ = unsafe { app.set_messaging_timeout(AX_MESSAGING_TIMEOUT_SECS) };
    Some(app)
}

fn ax_copy_value(el: &AXUIElement, attr: &str) -> Option<CFRetained<ObjCFType>> {
    let attr_cf = ObjCFString::from_str(attr);
    let mut value: *const ObjCFType = std::ptr::null();
    // SAFETY: `value` is a valid out-pointer for CopyAttributeValue.
    let err = unsafe { el.copy_attribute_value(&attr_cf, NonNull::from(&mut value)) };
    if err != AXError::Success || value.is_null() {
        return None;
    }
    let ptr = NonNull::new(value.cast_mut())?;
    // SAFETY: AX returns a +1 CF object on success; adopt it as retained.
    Some(unsafe { CFRetained::retain(ptr) })
}

fn ax_attr_string(el: &AXUIElement, attr: &str) -> Option<String> {
    ax_copy_value(el, attr)?
        .downcast::<ObjCFString>()
        .ok()
        .map(|s| s.to_string())
}

fn ax_attr_bool(el: &AXUIElement, attr: &str) -> bool {
    ax_copy_value(el, attr)
        .and_then(|v| v.downcast::<CFBoolean>().ok())
        .is_some_and(|b| b.as_bool())
}

fn ax_attr_element(el: &AXUIElement, attr: &str) -> Option<CFRetained<AXUIElement>> {
    ax_copy_value(el, attr)?.downcast::<AXUIElement>().ok()
}

/// Elements of an AX array attribute (`AXWindows`, `AXChildren`).
fn ax_attr_elements(el: &AXUIElement, attr: &str) -> Vec<CFRetained<AXUIElement>> {
    let Some(value) = ax_copy_value(el, attr) else {
        return Vec::new();
    };
    let Ok(arr) = value.downcast::<ObjCFArray>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..arr.count() {
        // SAFETY: the array holds AXUIElement references for these attributes.
        let ptr = unsafe { arr.value_at_index(i) };
        if ptr.is_null() {
            continue;
        }
        // SAFETY: non-null element pointer borrowed from the owning array.
        let child: &AXUIElement = unsafe { &*(ptr.cast()) };
        out.push(child.retain());
    }
    out
}

fn ax_children(el: &AXUIElement) -> Vec<CFRetained<AXUIElement>> {
    ax_attr_elements(el, AX_ATTR_CHILDREN)
}

/// `CGWindowID` of an AX window element, so ids match the previous scheme.
fn ax_window_id(win: &AXUIElement) -> Option<u32> {
    let mut id: u32 = 0;
    // SAFETY: private-but-exported HIServices call with a valid out-pointer.
    let err = unsafe { _AXUIElementGetWindow(win as *const AXUIElement, &mut id) };
    (err == AXError::Success && id != 0).then_some(id)
}

// ---------------------------------------------------------------------------
// Classification (W4)
// ---------------------------------------------------------------------------

/// Dialog verdict + normalized class from an AX window's role/subrole/modal.
///
/// Pure so it is unit-testable without a live app.
#[must_use]
pub fn classify_ax_window(role: &str, subrole: &str, modal: bool) -> (Option<bool>, String) {
    let is_dialog = role == AX_ROLE_SHEET
        || subrole == AX_SUBROLE_DIALOG
        || subrole == AX_SUBROLE_SYSTEM_DIALOG
        || modal;
    if is_dialog {
        return (Some(true), CLASS_DIALOG.to_string());
    }
    if subrole == AX_SUBROLE_STANDARD_WINDOW {
        return (Some(false), CLASS_STANDARD_WINDOW.to_string());
    }
    // Unknown shape: no verdict, let the portable heuristics decide.
    let class = if subrole.is_empty() { role } else { subrole };
    (None, class.to_string())
}

/// Normalize an AX control role onto the portable classifier's vocabulary.
///
/// `classify::fill_content` and `policy::plan_ladder` match `"Button"` and
/// `"Static"`; the Windows UIA backend already normalizes to those names, so
/// macOS must too. Emitting raw `"AXButton"`/`"AXStaticText"` here is what made
/// `describe` return empty buttons and forced every dismissal down the `Close`
/// branch, silently ignoring an explicit `button:` argument.
#[must_use]
pub fn normalize_ax_role(role: &str, subrole: &str) -> Option<&'static str> {
    if AX_CHROME_BUTTON_SUBROLES.contains(&subrole) {
        return None;
    }
    if role == AX_ROLE_BUTTON || subrole.contains("Button") {
        return Some("Button");
    }
    if role == AX_ROLE_STATIC_TEXT || role == AX_ROLE_TEXT {
        return Some("Static");
    }
    None
}

// ---------------------------------------------------------------------------
// Facade implementation
// ---------------------------------------------------------------------------

/// Title reported for a window we can only see through CGWindowList.
pub const TITLE_AX_UNRESPONSIVE: &str = "(untitled - app not answering accessibility)";
/// Normalized class for a CGWindowList-only fallback detection.
pub const CLASS_AX_UNRESPONSIVE: &str = "AXUnresponsive";

/// Window layer above which a window is a panel/dialog rather than a document
/// window. Normal document windows sit at layer 0; macOS floats modal and
/// utility panels above it (TD's dialogs were live-recorded at layer 8).
const CG_FLOATING_LAYER: i64 = 0;

/// Enumerate the AX windows owned by `pid`.
///
/// Falls back to CGWindowList when AX yields nothing. That fallback is the
/// whole reason detection survives the case it exists for: a main thread wedged
/// behind a modal stops answering AX entirely (live-recorded 2026-08-30 - a
/// `ui.messageBox` called from a worker thread deadlocked TouchDesigner, AX
/// returned zero windows, and an AX-only backend reported `popups=0,
/// windowStatus=None`, i.e. completely blind exactly when it mattered).
/// CGWindowList runs out-of-process and still enumerated the dialog.
pub fn top_level_windows(pid: u32) -> std::io::Result<Vec<SysWindow>> {
    let Some(app) = ax_app(pid) else {
        return Ok(Vec::new());
    };
    let ax_windows = ax_attr_elements(&app, AX_ATTR_WINDOWS);
    if ax_windows.is_empty() {
        return Ok(cg_fallback_windows(pid));
    }
    let mut out = Vec::new();
    for (i, win) in ax_windows.iter().enumerate() {
        let role = ax_attr_string(win, AX_ATTR_ROLE).unwrap_or_default();
        let subrole = ax_attr_string(win, AX_ATTR_SUBROLE).unwrap_or_default();
        let modal = ax_attr_bool(win, AX_ATTR_MODAL);
        let (is_dialog, class) = classify_ax_window(&role, &subrole, modal);
        out.push(SysWindow {
            pid,
            id: ax_window_id(win).map_or_else(|| format!("axidx-{i}"), |id| id.to_string()),
            class,
            title: ax_attr_string(win, AX_ATTR_TITLE).unwrap_or_default(),
            visible: true,
            styles: 0,
            ex_styles: 0,
            is_dialog,
        });
    }
    Ok(out)
}

/// Degraded enumeration for an app that has stopped answering AX.
///
/// Only elevated-layer windows are reported: they are the panels/dialogs. A
/// window-less or merely slow app therefore yields nothing rather than phantom
/// popups that would wedge the interception gate.
fn cg_fallback_windows(pid: u32) -> Vec<SysWindow> {
    let array = cg_window_list();
    let mut out = Vec::new();
    let mut saw_any = false;
    for i in 0..array.len() {
        let Some(dict) = array.get(i) else { continue };
        if cf_dict_get_number(&dict, "kCGWindowOwnerPID") != Some(i64::from(pid)) {
            continue;
        }
        saw_any = true;
        let layer = cf_dict_get_number(&dict, "kCGWindowLayer").unwrap_or(0);
        if layer <= CG_FLOATING_LAYER {
            continue;
        }
        let Some(id) = cf_dict_get_number(&dict, "kCGWindowNumber") else {
            continue;
        };
        out.push(SysWindow {
            pid,
            id: id.to_string(),
            class: CLASS_AX_UNRESPONSIVE.to_string(),
            title: TITLE_AX_UNRESPONSIVE.to_string(),
            visible: true,
            styles: layer as isize,
            ex_styles: 0,
            is_dialog: Some(true),
        });
    }
    if !out.is_empty() {
        tracing::warn!(
            pid,
            windows = out.len(),
            "app not answering accessibility - degraded CGWindowList detection"
        );
    } else if saw_any {
        tracing::debug!(pid, "no AX windows and no elevated-layer windows");
    }
    out
}

/// Resolve a window id back to its AX element by exact `CGWindowID` match.
fn ax_window_for_id(window_id: &str) -> Option<CFRetained<AXUIElement>> {
    let pid = pid_for_window_id(window_id)?;
    let app = ax_app(pid)?;
    let target = window_id.parse::<u32>().ok();
    ax_attr_elements(&app, AX_ATTR_WINDOWS)
        .into_iter()
        .find(|w| target.is_some() && ax_window_id(w) == target)
}

fn walk_ax_controls(
    el: &AXUIElement,
    default_label: Option<&str>,
    out: &mut Vec<SysControl>,
    depth: u32,
) {
    if depth > AX_WALK_DEPTH {
        return;
    }
    let role = ax_attr_string(el, AX_ATTR_ROLE).unwrap_or_default();
    let subrole = ax_attr_string(el, AX_ATTR_SUBROLE).unwrap_or_default();
    if let Some(class) = normalize_ax_role(&role, &subrole) {
        let label = ax_attr_string(el, AX_ATTR_TITLE)
            .or_else(|| ax_attr_string(el, AX_ATTR_VALUE))
            .unwrap_or_default();
        let ctrl_id = i32::try_from(out.len())
            .unwrap_or(i32::MAX)
            .saturating_add(1);
        let is_default =
            class == "Button" && default_label.is_some_and(|d| !d.is_empty() && d == label);
        out.push(SysControl {
            id: format!("ax-{ctrl_id}"),
            class: class.to_string(),
            label,
            ctrl_id: Some(ctrl_id),
            is_default,
        });
    }
    for child in ax_children(el) {
        walk_ax_controls(&child, default_label, out, depth + 1);
    }
}

/// Accessibility-tree controls for one window id (`CGWindowID`).
pub fn child_controls(id: &str) -> Vec<SysControl> {
    let Some(win) = ax_window_for_id(id) else {
        return Vec::new();
    };
    // The window itself names its default button; AX exposes no per-button
    // "is default" flag, so match by label.
    let default_label = ax_attr_element(&win, AX_ATTR_DEFAULT_BUTTON)
        .and_then(|b| ax_attr_string(&b, AX_ATTR_TITLE));
    let mut out = Vec::new();
    walk_ax_controls(&win, default_label.as_deref(), &mut out, 0);
    out
}

fn ax_press(el: &AXUIElement) -> bool {
    let action = ObjCFString::from_str(AX_ACTION_PRESS);
    // SAFETY: AXPress on a live element; errors are reported, not fatal.
    unsafe { el.perform_action(&action) == AXError::Success }
}

/// Press the nth normalized control (1-based `ctrl_id`) in the window subtree.
fn press_nth_control(win: &AXUIElement, ctrl_id: i32) -> bool {
    fn walk(el: &AXUIElement, want: i32, seen: &mut i32, depth: u32) -> bool {
        if depth > AX_WALK_DEPTH {
            return false;
        }
        let role = ax_attr_string(el, AX_ATTR_ROLE).unwrap_or_default();
        let subrole = ax_attr_string(el, AX_ATTR_SUBROLE).unwrap_or_default();
        if normalize_ax_role(&role, &subrole).is_some() {
            *seen += 1;
            if *seen == want {
                return ax_press(el);
            }
        }
        for child in ax_children(el) {
            if walk(&child, want, seen, depth + 1) {
                return true;
            }
        }
        false
    }
    let mut seen = 0;
    walk(win, ctrl_id, &mut seen, 0)
}

/// Click a control by `ctrl_id` within the window's AX tree.
///
/// Indexes the same normalized walk `child_controls` produces, so a ctrl id
/// taken from `describe` presses the button it named — no label round-trip.
pub fn post_click(id: &str, ctrl_id: i32) -> bool {
    let Some(win) = ax_window_for_id(id) else {
        return false;
    };
    press_nth_control(&win, ctrl_id)
}

/// Close fallback: the window's own cancel button, else a cancel-ish label.
pub fn post_close(id: &str) -> bool {
    let Some(win) = ax_window_for_id(id) else {
        return false;
    };
    if let Some(cancel) = ax_attr_element(&win, AX_ATTR_CANCEL_BUTTON) {
        if ax_press(&cancel) {
            return true;
        }
    }
    let controls = child_controls(id);
    let hit = controls.iter().find(|c| {
        c.class == "Button"
            && (c.label.eq_ignore_ascii_case("close")
                || c.label.eq_ignore_ascii_case("cancel")
                || c.label.eq_ignore_ascii_case("ok"))
    });
    if let Some(cid) = hit.and_then(|c| c.ctrl_id) {
        if press_nth_control(&win, cid) {
            return true;
        }
    }
    // Last resort: the window's own close widget. Excluded from `child_controls`
    // on purpose (it is chrome, not a dialog answer) but it is exactly what
    // "close the window" means - and for TouchDesigner's own dialogs it is the
    // only element AX exposes at all.
    press_chrome_close(&win)
}

/// Press the window-frame close widget, if the window has one.
fn press_chrome_close(win: &AXUIElement) -> bool {
    for child in ax_children(win) {
        if ax_attr_string(&child, AX_ATTR_SUBROLE).as_deref() == Some(AX_SUBROLE_CLOSE_BUTTON) {
            return ax_press(&child);
        }
    }
    false
}

/// Hang probe: a bounded AX round-trip that the app must answer on its main
/// thread. Timeout / `CannotComplete` means the app is not pumping (W6).
pub fn is_hung(id: &str, budget_ms: u32) -> bool {
    let Some(pid) = pid_for_window_id(id) else {
        return false;
    };
    if !accessibility_trusted() {
        return false;
    }
    let Ok(pid_i) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: plain pid; AX errors on dead processes rather than faulting.
    let app = unsafe { AXUIElement::new_application(pid_i) };
    let budget = (budget_ms as f32 / 1000.0).clamp(0.05, 2.0);
    // SAFETY: bounds the probe below so a wedged app cannot block us.
    let _ = unsafe { app.set_messaging_timeout(budget) };
    let attr = ObjCFString::from_str(AX_ATTR_WINDOWS);
    let mut value: *const ObjCFType = std::ptr::null();
    // SAFETY: valid out-pointer; the call is bounded by the timeout above.
    let err = unsafe { app.copy_attribute_value(&attr, NonNull::from(&mut value)) };
    if !value.is_null() {
        // SAFETY: adopt the +1 result so it is not leaked.
        drop(unsafe { CFRetained::retain(NonNull::new_unchecked(value.cast_mut())) });
    }
    matches!(err, AXError::CannotComplete)
}

/// Image basename of `pid`, e.g. `"TouchDesigner"`.
pub fn process_image_name(pid: u32) -> Option<String> {
    let mut mib: [c_int; 4] = [
        libc::CTL_KERN,
        KERN_PROC,
        KERN_PROC_PIDPATH,
        c_int::try_from(pid).ok()?,
    ];
    let mut buf = [0u8; 4096];
    let mut size = buf.len();
    // SAFETY: sysctl with a valid 4-element mib and a sized output buffer.
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

/// True when `pid` is a zombie: exited, but not yet reaped by its parent.
///
/// A zombie keeps its PID table entry, so `kill(pid, 0)` still succeeds for it.
/// TouchDesigner spawned by the daemon is our own child, so a clean exit parks
/// it in this state until reaped — without this check a graceful `kill_td`
/// reports `graceful_timeout` for a TD that actually shut down correctly.
/// The kernel drops a zombie's BSD info while keeping its PID entry, so
/// `proc_pidinfo` fails with `ESRCH` for it and succeeds for every live process
/// — including ones we do not own (verified against root-owned `launchd`).
/// `p_stat`/`SZOMB` is never actually observable this way: the call fails first.
fn process_zombie(pid: u32) -> bool {
    let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as c_int;
    // SAFETY: `info` is a correctly-sized, owned buffer for PROC_PIDT_SHORTBSDINFO.
    let rc = unsafe {
        *libc::__error() = 0;
        libc::proc_pidinfo(
            pid as c_int,
            libc::PROC_PIDT_SHORTBSDINFO,
            0,
            std::ptr::addr_of_mut!(info).cast(),
            size,
        )
    };
    if rc == size {
        return false; // full BSD info -> a live process
    }
    // Only ESRCH means "no such live process". Any other failure (short read,
    // an unexpected errno) fails open: never report a running TD as gone.
    // SAFETY: reading thread-local errno set by the call above.
    unsafe { *libc::__error() == libc::ESRCH }
}

/// Cheap liveness probe via signal 0, excluding unreaped zombies.
pub fn process_alive(pid: u32) -> bool {
    let Ok(pid_i) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: kill(pid, 0) sends no signal; it only checks existence.
    if unsafe { kill(pid_i, 0) != 0 } {
        return false;
    }
    !process_zombie(pid)
}

/// Post close to every window of `pid` (graceful kill helper).
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
        let Ok(pid_i) = i32::try_from(pid) else {
            return 0;
        };
        // SAFETY: graceful terminate signal to a plain pid.
        if unsafe { kill(pid_i, libc::SIGTERM) == 0 } {
            sent = 1;
        }
    }
    sent
}

/// Hard-terminate via SIGKILL.
pub fn terminate_process(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: SIGKILL is the Unix last-resort terminate.
    unsafe { kill(pid, libc::SIGKILL) == 0 }
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

    /// Regression: an exited-but-unreaped child must read as dead.
    ///
    /// `kill(pid, 0)` succeeds for a zombie, so the old probe reported a
    /// cleanly-exited TouchDesigner as still alive and `kill_td` graceful
    /// answered `graceful_timeout` instead of success.
    #[test]
    fn process_alive_false_for_unreaped_zombie() {
        let mut child = std::process::Command::new("/usr/bin/true")
            .spawn()
            .expect("spawn /usr/bin/true");
        let pid = child.id();

        // Wait for it to exit without reaping it (no wait()/try_wait()).
        let mut zombie = false;
        for _ in 0..200 {
            if process_zombie(pid) {
                zombie = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(zombie, "child never became an unreaped zombie");

        // kill(pid, 0) still succeeds here — that is exactly the trap.
        assert!(unsafe { kill(pid as i32, 0) } == 0);
        assert!(!process_alive(pid), "zombie must not count as alive");

        let _ = child.wait(); // reap
    }

    #[test]
    fn sheet_and_dialog_subroles_classify_as_dialogs() {
        assert_eq!(
            classify_ax_window(AX_ROLE_SHEET, "", false),
            (Some(true), CLASS_DIALOG.to_string())
        );
        assert_eq!(
            classify_ax_window("AXWindow", AX_SUBROLE_DIALOG, false),
            (Some(true), CLASS_DIALOG.to_string())
        );
        assert_eq!(
            classify_ax_window("AXWindow", AX_SUBROLE_SYSTEM_DIALOG, false),
            (Some(true), CLASS_DIALOG.to_string())
        );
    }

    #[test]
    fn modal_flag_alone_classifies_as_dialog() {
        assert_eq!(
            classify_ax_window("AXWindow", AX_SUBROLE_STANDARD_WINDOW, true),
            (Some(true), CLASS_DIALOG.to_string())
        );
    }

    #[test]
    fn standard_window_is_explicitly_not_a_dialog() {
        // The verdict must be Some(false), not None: this is what keeps TD's
        // main editor window out of the interception gate.
        assert_eq!(
            classify_ax_window("AXWindow", AX_SUBROLE_STANDARD_WINDOW, false),
            (Some(false), CLASS_STANDARD_WINDOW.to_string())
        );
    }

    #[test]
    fn unknown_shape_defers_to_portable_heuristics() {
        let (verdict, class) = classify_ax_window("AXWindow", "", false);
        assert_eq!(verdict, None);
        assert_eq!(class, "AXWindow");
    }

    #[test]
    fn ax_roles_normalize_onto_the_portable_vocabulary() {
        // Regression: raw "AXButton"/"AXStaticText" never matched
        // classify::fill_content or policy::plan_ladder.
        assert_eq!(normalize_ax_role("AXButton", ""), Some("Button"));
        assert_eq!(normalize_ax_role("AXStaticText", ""), Some("Static"));
        assert_eq!(normalize_ax_role("AXText", ""), Some("Static"));
        assert_eq!(normalize_ax_role("AXGroup", ""), None);
    }

    #[test]
    fn window_chrome_widgets_are_not_dialog_buttons() {
        // Live-recorded regression: a TD `ui.messageBox` exposes ONLY these
        // three buttons to AX. Reporting them as dialog buttons made `describe`
        // advertise three unlabeled buttons and let a dismissal press zoom or
        // minimize instead of answering the dialog.
        for subrole in ["AXCloseButton", "AXZoomButton", "AXMinimizeButton"] {
            assert_eq!(
                normalize_ax_role("AXButton", subrole),
                None,
                "{subrole} must not be reported as a dialog button"
            );
        }
        // A real content button still normalizes.
        assert_eq!(normalize_ax_role("AXButton", ""), Some("Button"));
    }
}
