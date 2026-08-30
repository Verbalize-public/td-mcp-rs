//! Live macOS dialog tests against a real TouchDesigner process.
//!
//! All `#[ignore]`d: they need TCC Accessibility plus a running TD. Run with
//! ```text
//! TD_PID=<pid> cargo test -p tdmcp-dialogs --test live_permissions -- --ignored --nocapture
//! ```
//! The TCC grant follows the *terminal* these are launched from (the responsible
//! process), so no separate grant for the test binary is needed and rebuilds do
//! not invalidate it.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "live tests")]

use tdmcp_core::DialogSource;
use tdmcp_dialogs::sys::macos;

fn td_pid() -> u32 {
    std::env::var("TD_PID")
        .expect("set TD_PID to a running TouchDesigner pid")
        .parse()
        .expect("TD_PID must be numeric")
}

#[test]
#[ignore = "live: requires a running TouchDesigner and TCC Accessibility"]
fn accessibility_is_granted() {
    assert!(
        macos::accessibility_trusted(),
        "Accessibility not granted to the terminal running these tests"
    );
}

/// Regression for the root bug: titles used to come from `kCGWindowName`, which
/// is empty without Screen Recording, so every window was filtered as chrome and
/// macOS detected zero popups. AX must return the real title with Accessibility
/// alone.
#[test]
#[ignore = "live: requires a running TouchDesigner and TCC Accessibility"]
fn ax_enumeration_returns_real_titles_without_screen_recording() {
    let pid = td_pid();
    let windows = macos::top_level_windows(pid).expect("enumerate");
    assert!(!windows.is_empty(), "no AX windows for pid {pid}");
    for w in &windows {
        println!(
            "  id={} class={} is_dialog={:?} title={:?}",
            w.id, w.class, w.is_dialog, w.title
        );
    }
    let main = windows
        .iter()
        .find(|w| w.title.to_lowercase().starts_with("touchdesigner"))
        .expect("no window carried a TouchDesigner title - AX titles regressed");
    assert!(
        !main.title.is_empty(),
        "main window title empty: the Screen-Recording-dependent path is back"
    );
    assert_eq!(
        main.is_dialog,
        Some(false),
        "TD's standard editor window must be an explicit non-dialog"
    );
    assert!(
        main.id.parse::<u32>().is_ok(),
        "id {} is not a CGWindowID - _AXUIElementGetWindow failed",
        main.id
    );
}

/// An idle TD must produce no popups: the interception gate has to stay quiet,
/// or every bridged tool call fails with `tdmcp.dialog.blocking`.
#[test]
#[ignore = "live: requires a running TouchDesigner with no modal open"]
fn idle_touchdesigner_yields_no_false_positive_popups() {
    let pid = td_pid();
    let src = tdmcp_dialogs::MacDialogSource::new();
    let snap = src.snapshot(pid);
    println!(
        "  windowStatus={:?} popups={}",
        snap.window_status,
        snap.popups.len()
    );
    for p in &snap.popups {
        println!(
            "    popup id={} title={:?} class={:?}",
            p.id, p.title, p.class
        );
    }
    assert!(
        snap.popups.is_empty(),
        "idle TD reported {} popup(s) - false positives will wedge the gate",
        snap.popups.len()
    );
    assert_eq!(
        snap.window_status,
        Some(tdmcp_core::WindowStatus::Responsive),
        "idle TD should probe as responsive"
    );
}

/// The snapshot path must stay inside the watcher budget; it runs every
/// `[dialogs].poll_ms` against every registered pid.
#[test]
#[ignore = "live: requires a running TouchDesigner"]
fn snapshot_stays_within_budget() {
    let pid = td_pid();
    let src = tdmcp_dialogs::MacDialogSource::new();
    let _warm = src.snapshot(pid);
    let mut worst = std::time::Duration::ZERO;
    for _ in 0..5 {
        std::thread::sleep(tdmcp_dialogs::CACHE_TTL);
        let t = std::time::Instant::now();
        let _ = src.snapshot(pid);
        worst = worst.max(t.elapsed());
    }
    println!("  worst uncached snapshot = {worst:?}");
    assert!(
        worst < tdmcp_dialogs::SNAPSHOT_BUDGET * 2,
        "snapshot {worst:?} far exceeds the {:?} budget",
        tdmcp_dialogs::SNAPSHOT_BUDGET
    );
}
