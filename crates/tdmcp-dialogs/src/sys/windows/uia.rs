//! UIA COM content fill-in for Qt-hosted dialogs where classic controls come
//! up empty (docs/archive/DIALOGS.md §4). Runs ONLY on the dialogs worker thread; COM is
//! initialized once there. Fail-open: every failure degrades to no data.

#![allow(clippy::undocumented_unsafe_blocks)]
// SAFETY-bearing module: raw COM via windows-rs, confined to the enclave per
// RISKS.md R9. Public surface is safe and returns plain tuples.

use windows::core::Interface;
use windows::Win32::Foundation::{BOOL, HWND, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationElementArray,
    IUIAutomationInvokePattern, UIA_ButtonControlTypeId, UIA_InvokePatternId,
    UIA_TextControlTypeId,
};

use super::SysControl;

thread_local! {
    static COM_INIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn automation() -> Option<IUIAutomation> {
    if !COM_INIT.with(|c| c.get()) {
        // SAFETY: COM apartment init on the dedicated worker thread only.
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            if hr.is_err() && hr != RPC_E_CHANGED_MODE {
                tracing::warn!(?hr, "CoInitializeEx failed - UIA disabled");
                return None;
            }
        }
        COM_INIT.with(|c| c.set(true));
    }
    // SAFETY: standard in-proc COM activation of the UIA client.
    unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok() }
}

/// One accessibility-tree entry flattened into facade terms.
fn element_entry(elem: &IUIAutomationElement) -> Option<SysControl> {
    // SAFETY: property getters on a live element pointer.
    unsafe {
        let name = elem.CurrentName().ok()?.to_string();
        if name.is_empty() {
            return None;
        }
        let ctype = elem.CurrentControlType().ok()?;
        let class = if ctype == UIA_ButtonControlTypeId {
            "Button"
        } else if ctype == UIA_TextControlTypeId {
            "Static"
        } else {
            "UiaOther"
        };
        let is_default = elem
            .CurrentIsKeyboardFocusable()
            .map(bool_of)
            .unwrap_or(false);
        Some(SysControl {
            id: format!("uia:{name}"),
            class: class.into(),
            label: name.clone(),
            ctrl_id: None,
            is_default,
        })
    }
}

fn bool_of(b: BOOL) -> bool {
    b.as_bool()
}

/// Accessibility-tree children of `id` (hwnd-derived), deduped by caller.
pub fn child_controls(id: &str) -> Vec<SysControl> {
    let Some(hwnd_val) = id.parse::<isize>().ok() else {
        return Vec::new();
    };
    let Some(au) = automation() else {
        return Vec::new();
    };
    // SAFETY: element lookup + subtree read on a live hwnd handle.
    unsafe {
        let elem = match au.ElementFromHandle(HWND(hwnd_val as *mut _)) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        let Ok(cond) = au.CreateTrueCondition() else {
            return Vec::new();
        };
        // SAFETY: subtree enumeration on our own element pointer; true
        // condition borrows only COM-owned state.
        let Ok(array) = elem.FindAll(windows::Win32::UI::Accessibility::TreeScope_Children, &cond)
        else {
            return Vec::new();
        };
        flatten(&array)
    }
}

unsafe fn flatten(array: &IUIAutomationElementArray) -> Vec<SysControl> {
    // SAFETY: array accessors within bounds from Length.
    let len = array.Length().unwrap_or(0);
    let mut out = Vec::with_capacity(len as usize);
    for i in 0..len {
        if let Ok(e) = array.GetElement(i) {
            if let Some(entry) = element_entry(&e) {
                out.push(entry);
            }
        }
    }
    out
}

/// Invoke a named button via UIA when no classic ctrl id exists.
pub fn press_named(id: &str, label: &str) -> bool {
    for c in child_controls(id) {
        if c.class == "Button" && c.label.eq_ignore_ascii_case(label) {
            return invoke_element(id, &c.label);
        }
    }
    false
}

fn invoke_element(hwnd_id: &str, _label: &str) -> bool {
    let Some(hwnd_val) = hwnd_id.parse::<isize>().ok() else {
        return false;
    };
    let Some(au) = automation() else {
        return false;
    };
    // SAFETY: pattern cast + Invoke on a live element.
    unsafe {
        let Ok(elem) = au.ElementFromHandle(HWND(hwnd_val as *mut _)) else {
            return false;
        };
        // SAFETY: pattern retrieval + Invoke on a live element - post-free
        // UIA call that never blocks on the target thread.
        let Ok(pattern) = elem.GetCurrentPattern(UIA_InvokePatternId) else {
            return false;
        };
        let Ok(invoke) = pattern.cast::<IUIAutomationInvokePattern>() else {
            return false;
        };
        invoke.Invoke().is_ok()
    }
}
