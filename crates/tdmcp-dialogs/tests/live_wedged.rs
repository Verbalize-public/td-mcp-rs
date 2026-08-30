//! Diagnostic: what is still observable when the target app stops answering AX.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "diagnostic")]

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::window::{
    kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionAll,
    CGWindowListCopyWindowInfo,
};
use tdmcp_core::DialogSource;

#[test]
#[ignore = "diagnostic"]
fn what_survives_a_wedged_app() {
    let pid: u32 = std::env::var("TD_PID").unwrap().parse().unwrap();

    println!(
        "accessibility_trusted = {}",
        tdmcp_dialogs::sys::macos::accessibility_trusted()
    );
    let t = std::time::Instant::now();
    let ax = tdmcp_dialogs::sys::macos::top_level_windows(pid).unwrap();
    println!(
        "AX top_level_windows -> {} window(s) in {:?}",
        ax.len(),
        t.elapsed()
    );

    let t = std::time::Instant::now();
    let hung = tdmcp_dialogs::sys::macos::is_hung("0", 800);
    println!("is_hung(bogus id) = {hung} in {:?}", t.elapsed());

    let src = tdmcp_dialogs::MacDialogSource::new();
    let t = std::time::Instant::now();
    let snap = src.snapshot(pid);
    println!(
        "snapshot -> popups={} windowStatus={:?} in {:?}",
        snap.popups.len(),
        snap.window_status,
        t.elapsed()
    );

    // CGWindowList is out-of-process: it should still see the windows.
    let list = unsafe {
        CGWindowListCopyWindowInfo(
            kCGWindowListOptionAll | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        )
    };
    let arr: CFArray<CFDictionary<CFString, CFType>> =
        unsafe { CFArray::wrap_under_create_rule(list) };
    println!("CGWindowList windows owned by pid {pid}:");
    for i in 0..arr.len() {
        let d = arr.get(i).unwrap();
        let owner = d
            .find(CFString::new("kCGWindowOwnerPID"))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())
            .unwrap_or(-1);
        if owner != i64::from(pid) {
            continue;
        }
        let num = d
            .find(CFString::new("kCGWindowNumber"))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())
            .unwrap_or(-1);
        let layer = d
            .find(CFString::new("kCGWindowLayer"))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())
            .unwrap_or(-1);
        let name = d
            .find(CFString::new("kCGWindowName"))
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string());
        let alpha = d
            .find(CFString::new("kCGWindowAlpha"))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_f64())
            .unwrap_or(-1.0);
        println!("  num={num} layer={layer} alpha={alpha} name={name:?}");
    }
}
