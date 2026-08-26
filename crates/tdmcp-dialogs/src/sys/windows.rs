//! Windows user32 backend — the quarantined `unsafe` enclave.
//!
//! Every public function here is safe; each wraps FFI with `// SAFETY:`
//! comments. UIA COM content fill-in lands next (A3b) in `sys/windows/uia.rs`
//! on this same worker thread.

#![allow(clippy::undocumented_unsafe_blocks)]
// The shim intentionally reads as raw Win32; dead-code allowances mark helpers
// reserved for A3b (owner-chain corroboration).
#![allow(dead_code)]

use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetDlgCtrlID, GetWindowLongPtrW,
    GetWindowTextLengthW, GetWindowTextW, IsWindowVisible, PostMessageW, SendMessageTimeoutW,
    BM_CLICK, BS_DEFPUSHBUTTON, GWL_EXSTYLE, GWL_STYLE, SMTO_ABORTIFHUNG, WM_CLOSE, WM_NULL,
};

use super::{SysControl, SysWindow};

pub mod uia;

fn hwnd_id(hwnd: HWND) -> String {
    (hwnd.0 as isize).to_string()
}

fn parse_hwnd(id: &str) -> Option<HWND> {
    id.parse::<isize>().ok().map(|v| HWND(v as *mut _))
}

// SAFETY contract shared by both enum callbacks: EnumWindows/EnumChildWindows
// are synchronous — the context pointer passed via LPARAM outlives every call.
struct WinCtx {
    pid: u32,
    out: Vec<SysWindow>,
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut WinCtx);
    let mut owner_pid = 0u32;
    let _ = GetWindowThreadProcessId(hwnd, Some(&mut owner_pid));
    if owner_pid != ctx.pid {
        return BOOL(1); // different process - keep enumerating
    }
    // SAFETY: read-only queries on an enumerated live handle; context outlives
    // the synchronous enumeration.
    let visible = IsWindowVisible(hwnd).as_bool();
    let class = read_class(hwnd);
    let title = read_text(hwnd);
    let styles = GetWindowLongPtrW(hwnd, GWL_STYLE);
    let ex_styles = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    ctx.out.push(SysWindow {
        pid: ctx.pid,
        id: hwnd_id(hwnd),
        class,
        title,
        visible,
        styles,
        ex_styles,
    });
    BOOL(1)
}

fn read_text(hwnd: HWND) -> String {
    // Bounded window-text read on a live enumerated handle.
    unsafe {
        let len = GetWindowTextLengthW(hwnd).max(0) as usize;
        if len == 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len + 1];
        let copied = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..copied.max(0) as usize])
    }
}

fn read_class(hwnd: HWND) -> String {
    // Bounded class-name read.
    unsafe {
        let mut buf = [0u16; 256];
        let copied = GetClassNameW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..copied.max(0) as usize])
    }
}

/// Enumerate visible top-level windows owned by `pid`.
pub fn top_level_windows(pid: u32) -> std::io::Result<Vec<SysWindow>> {
    let mut ctx = WinCtx {
        pid,
        out: Vec::new(),
    };
    let lparam = LPARAM(&mut ctx as *mut WinCtx as isize);
    // SAFETY: synchronous enumeration with stack-owned context (see above).
    unsafe {
        EnumWindows(
            Some(enum_proc as unsafe extern "system" fn(HWND, LPARAM) -> BOOL),
            lparam,
        )
    }
    .map(|_| ctx.out)
    .map_err(|e| std::io::Error::other(e.to_string()))
}

struct KidCtx {
    out: Vec<SysControl>,
}

unsafe extern "system" fn child_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: context pointer validity mirrors enum_proc (synchronous walk).
    let kids = &mut *(lparam.0 as *mut KidCtx);
    let class = read_class(hwnd);
    let label = read_text(hwnd);
    let ctrl_id = GetDlgCtrlID(hwnd);
    let is_default = (GetWindowLongPtrW(hwnd, GWL_STYLE) & BS_DEFPUSHBUTTON as isize) != 0;
    kids.out.push(SysControl {
        id: hwnd_id(hwnd),
        class,
        label,
        ctrl_id: Some(ctrl_id),
        is_default,
    });
    BOOL(1)
}

/// Child controls of a dialog window: classic controls first, UIA
/// accessibility fill-in appended (deduped by class+label).
pub fn child_controls(id: &str) -> Vec<SysControl> {
    let Some(hwnd) = parse_hwnd(id) else {
        return Vec::new();
    };
    let mut kids = KidCtx { out: Vec::new() };
    let lparam = LPARAM(&mut kids as *mut KidCtx as isize);
    // SAFETY: synchronous enumeration with stack-owned context.
    unsafe {
        let _ = EnumChildWindows(
            hwnd,
            Some(child_proc as unsafe extern "system" fn(HWND, LPARAM) -> BOOL),
            lparam,
        );
    }
    let had_buttons = kids
        .out
        .iter()
        .any(|c| c.class.eq_ignore_ascii_case("Button"));
    let had_message = kids
        .out
        .iter()
        .any(|c| c.class.eq_ignore_ascii_case("Static") && !c.label.is_empty());
    if !(had_buttons && had_message) {
        for u in uia::child_controls(id) {
            let dup = kids
                .out
                .iter()
                .any(|c| c.class == u.class && c.label == u.label);
            if !dup {
                kids.out.push(u);
            }
        }
    }
    kids.out
}

struct FindCtx {
    want: i32,
    hit: Option<HWND>,
}

unsafe extern "system" fn find_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let f = &mut *(lparam.0 as *mut FindCtx);
    // SAFETY: ctrl-id query on enumerated live handle.
    if GetDlgCtrlID(hwnd) == f.want {
        f.hit = Some(hwnd);
        return BOOL(0); // stop
    }
    BOOL(1)
}

fn find_child_by_ctrl(parent: HWND, ctrl_id: i32) -> Option<HWND> {
    let mut f = FindCtx {
        want: ctrl_id,
        hit: None,
    };
    let lparam = LPARAM(&mut f as *mut FindCtx as isize);
    // SAFETY: synchronous enumeration with stack-owned context.
    unsafe {
        let _ = EnumChildWindows(
            parent,
            Some(find_proc as unsafe extern "system" fn(HWND, LPARAM) -> BOOL),
            lparam,
        );
    }
    f.hit
}

/// Post `BM_CLICK` to the child control with `ctrl_id` inside dialog `id`.
///
/// Post-only (no focus steal) and non-blocking by design — the target thread
/// may be wedged; verify-gone happens at the policy layer.
pub fn post_click(id: &str, ctrl_id: i32) -> bool {
    let Some(hwnd) = parse_hwnd(id) else {
        return false;
    };
    let Some(btn) = find_child_by_ctrl(hwnd, ctrl_id) else {
        return false;
    };
    // SAFETY: post-only message delivery.
    unsafe { PostMessageW(btn, BM_CLICK, WPARAM(0), LPARAM(0)).is_ok() }
}

/// Post WM_CLOSE to the dialog itself.
pub fn post_close(id: &str) -> bool {
    let Some(hwnd) = parse_hwnd(id) else {
        return false;
    };
    // SAFETY: post-only message delivery.
    unsafe { PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok() }
}

/// Hang probe: true when the window did not answer within `budget_ms`.
pub fn is_hung(id: &str, budget_ms: u32) -> bool {
    let Some(hwnd) = parse_hwnd(id) else {
        return false;
    };
    let mut result = 0usize;
    // SAFETY: benign WM_NULL ping with SMTO_ABORTIFHUNG - cannot hang ourselves.
    unsafe {
        let r = SendMessageTimeoutW(
            hwnd,
            WM_NULL,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            budget_ms,
            Some(&mut result),
        );
        r.0 == 0 || result == 0
    }
}

/// Image basename of `pid`, e.g. `"TouchDesigner.exe"` — kill_td pid check.
pub fn process_image_name(pid: u32) -> Option<String> {
    // SAFETY: query-limited handle, closed on every path below.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let out = if res.is_ok() && len > 0 {
            String::from_utf16_lossy(&buf[..len as usize])
                .rsplit(['\\', '/'])
                .next()
                .map(str::to_string)
        } else {
            None
        };
        let _ = CloseHandle(handle);
        out
    }
}

use windows::Win32::Foundation::{CloseHandle, WPARAM};

/// Cheap liveness probe: a queryable handle means the process exists.
pub fn process_alive(pid: u32) -> bool {
    // SAFETY: query-limited handle, closed immediately.
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                true
            }
            Err(_) => false,
        }
    }
}

/// Post WM_CLOSE to every visible top-level window of `pid`.
/// Returns how many windows received the message (0 => nothing to close).
pub fn close_pid_windows(pid: u32) -> usize {
    let mut ctx = WinCtx {
        pid,
        out: Vec::new(),
    };
    let lparam = LPARAM(&mut ctx as *mut WinCtx as isize);
    // SAFETY: synchronous enumeration with stack-owned context.
    unsafe {
        let _ = EnumWindows(
            Some(enum_proc as unsafe extern "system" fn(HWND, LPARAM) -> BOOL),
            lparam,
        );
    }
    let mut sent = 0usize;
    for w in &ctx.out {
        if let Some(h) = parse_hwnd(&w.id) {
            // SAFETY: post-only delivery; non-blocking by design.
            if unsafe { PostMessageW(h, WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok() } {
                sent += 1;
            }
        }
    }
    sent
}

/// Hard-terminate `pid` (TerminateProcess). Last resort after graceful ladder.
pub fn terminate_process(pid: u32) -> bool {
    // SAFETY: PROCESS_TERMINATE handle closed on every path below.
    unsafe {
        match OpenProcess(PROCESS_TERMINATE, false, pid) {
            Ok(handle) => {
                let ok = TerminateProcess(handle, 1).is_ok();
                let _ = CloseHandle(handle);
                ok
            }
            Err(_) => false,
        }
    }
}
