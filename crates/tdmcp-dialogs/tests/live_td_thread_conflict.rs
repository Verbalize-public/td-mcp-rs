//! W9: real TouchDesigner warning dialogs (`THREAD CONFLICT`).
//!
//! TD raises this dialog when an operator is touched from a non-main thread.
//! Its title is one of `classify::HARD_MARKERS`, so this is the only test that
//! exercises the **Hard** severity path against a genuine TD dialog rather than
//! a synthesized one.
//!
//! Policy note: hard dialogs are "surface loudly, never click through". This
//! test therefore asserts the severity is surfaced FIRST, and only then cleans
//! up the windows it found.
//!
//! ```text
//! TD_PID=<pid> cargo test -p tdmcp-dialogs --test live_td_thread_conflict -- --ignored --nocapture
//! ```
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "live test")]

use tdmcp_core::{DialogSeverity, DialogSource};

#[test]
#[ignore = "live: requires TD showing at least one THREAD CONFLICT dialog"]
fn thread_conflict_dialogs_classify_as_hard() {
    let pid: u32 = std::env::var("TD_PID")
        .expect("TD_PID")
        .parse()
        .expect("numeric");
    let src = tdmcp_dialogs::MacDialogSource::new();
    let snap = src.snapshot(pid);
    println!(
        "  windowStatus={:?} popups={}",
        snap.window_status,
        snap.popups.len()
    );

    let conflicts: Vec<_> = snap
        .popups
        .iter()
        .filter(|p| p.title.eq_ignore_ascii_case("THREAD CONFLICT"))
        .collect();
    assert!(
        !conflicts.is_empty(),
        "no THREAD CONFLICT dialog present - trigger one first"
    );
    for p in &conflicts {
        println!("  id={} severity={:?} kind={:?}", p.id, p.severity, p.kind);
        assert_eq!(
            p.severity,
            DialogSeverity::Hard,
            "THREAD CONFLICT must classify as Hard (it is a HARD_MARKER)"
        );
        assert!(!p.is_main_chrome, "a TD warning dialog is not main chrome");
    }
    assert_eq!(
        snap.window_status,
        Some(tdmcp_core::WindowStatus::BlockedByModalWindow)
    );
    println!(
        "  {} THREAD CONFLICT dialog(s) surfaced as Hard",
        conflicts.len()
    );
}

/// Cleanup helper: clear every popup this pid is showing.
#[test]
#[ignore = "live: destructive cleanup - dismisses every open popup on the pid"]
fn dismiss_all_open_popups() {
    let pid: u32 = std::env::var("TD_PID")
        .expect("TD_PID")
        .parse()
        .expect("numeric");
    let src = tdmcp_dialogs::MacDialogSource::new();
    for round in 0..30 {
        let snap = src.snapshot(pid);
        let Some(p) = snap.popups.first().cloned() else {
            println!(
                "  clean after {round} round(s); windowStatus={:?}",
                snap.window_status
            );
            return;
        };
        match src.dismiss(pid, &p.id, None) {
            Ok(o) => println!("  dismissed {} ({:?}) via={:?}", p.id, p.title, o.via),
            Err(e) => {
                println!("  FAILED on {} ({:?}): {e:?}", p.id, p.title);
                break;
            }
        }
    }
    let left = src.snapshot(pid).popups.len();
    println!("  remaining popups = {left}");
}
