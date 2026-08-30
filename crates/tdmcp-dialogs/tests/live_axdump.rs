//! Diagnostic: dump the full AX subtree of an open TD modal, with every
//! attribute name and its string value. Used to work out which attributes Qt
//! actually populates for TouchDesigner's dialogs.
//!
//! ```text
//! TD_PID=<pid> cargo test -p tdmcp-dialogs --test live_axdump -- --ignored --nocapture
//! ```
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "diagnostic test")]

use std::ptr::NonNull;

use objc2_application_services::{AXError, AXUIElement};
use objc2_core_foundation::{CFArray, CFRetained, CFString, CFType, Type};

fn ax_copy(el: &AXUIElement, attr: &str) -> Option<CFRetained<CFType>> {
    let attr_cf = CFString::from_str(attr);
    let mut value: *const CFType = std::ptr::null();
    let err = unsafe { el.copy_attribute_value(&attr_cf, NonNull::from(&mut value)) };
    if err != AXError::Success || value.is_null() {
        return None;
    }
    Some(unsafe { CFRetained::retain(NonNull::new(value.cast_mut())?) })
}

fn ax_str(el: &AXUIElement, attr: &str) -> Option<String> {
    ax_copy(el, attr)?
        .downcast::<CFString>()
        .ok()
        .map(|s| s.to_string())
}

fn attr_names(el: &AXUIElement) -> Vec<String> {
    let mut names: *const CFArray = std::ptr::null();
    let err = unsafe { el.copy_attribute_names(NonNull::from(&mut names)) };
    if err != AXError::Success || names.is_null() {
        return Vec::new();
    }
    let arr = unsafe { CFRetained::retain(NonNull::new(names.cast_mut()).unwrap()) };
    let mut out = Vec::new();
    for i in 0..arr.count() {
        let ptr = unsafe { arr.value_at_index(i) };
        if ptr.is_null() {
            continue;
        }
        let s: &CFString = unsafe { &*(ptr.cast()) };
        out.push(s.to_string());
    }
    out
}

fn children(el: &AXUIElement) -> Vec<CFRetained<AXUIElement>> {
    let Some(v) = ax_copy(el, "AXChildren") else {
        return Vec::new();
    };
    let Ok(arr) = v.downcast::<CFArray>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..arr.count() {
        let ptr = unsafe { arr.value_at_index(i) };
        if ptr.is_null() {
            continue;
        }
        let c: &AXUIElement = unsafe { &*(ptr.cast()) };
        out.push(c.retain());
    }
    out
}

fn dump(el: &AXUIElement, depth: usize) {
    let pad = "  ".repeat(depth);
    let role = ax_str(el, "AXRole").unwrap_or_default();
    let subrole = ax_str(el, "AXSubrole").unwrap_or_default();
    println!("{pad}- role={role} subrole={subrole}");
    for name in attr_names(el) {
        if name == "AXChildren" || name == "AXParent" || name == "AXWindows" {
            continue;
        }
        if let Some(v) = ax_str(el, &name) {
            if !v.is_empty() {
                println!("{pad}    {name} = {v:?}");
            }
        }
    }
    if depth < 10 {
        for c in children(el) {
            dump(&c, depth + 1);
        }
    }
}

#[test]
#[ignore = "live: requires an OPEN modal dialog in TouchDesigner"]
fn dump_modal_ax_tree() {
    let pid: u32 = std::env::var("TD_PID")
        .expect("TD_PID")
        .parse()
        .expect("numeric");
    let app = unsafe { AXUIElement::new_application(pid as i32) };
    let _ = unsafe { app.set_messaging_timeout(2.0) };
    let v = ax_copy(&app, "AXWindows").expect("AXWindows");
    let arr = v.downcast::<CFArray>().expect("array");
    for i in 0..arr.count() {
        let ptr = unsafe { arr.value_at_index(i) };
        if ptr.is_null() {
            continue;
        }
        let w: &AXUIElement = unsafe { &*(ptr.cast()) };
        let subrole = ax_str(w, "AXSubrole").unwrap_or_default();
        if subrole == "AXStandardWindow" {
            println!("(skipping main editor window)");
            continue;
        }
        println!("=== window {i} ===");
        dump(w, 0);
    }
}
