//! Live modal round-trip: list → describe → dismiss against a real TD dialog.
//!
//! These are the direct regression tests for the AX control-class mismatch: the
//! macOS backend used to emit `AXButton`/`AXStaticText` while the portable
//! classifier matched `Button`/`Static`, so `describe` returned no buttons and
//! no message, and `plan_ladder` could never choose `Click` — every dismissal
//! silently fell through to the `Close` branch and an explicit `button:`
//! argument was ignored.
//!
//! Open a modal in TouchDesigner first, e.g. via the MCP `execute_python` tool:
//! ```text
//! run("ui.messageBox('tdmcp live', 'probe', buttons=['Cancel','OK'])", delayFrames=120)
//! ```
//! then:
//! ```text
//! TD_PID=<pid> cargo test -p tdmcp-dialogs --test live_modal -- --ignored --nocapture
//! ```
#![cfg(target_os = "macos")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "live tests"
)]

use std::time::{Duration, Instant};

use tdmcp_core::{DialogSource, PopupInfo};

/// How long to wait for a manually/remotely triggered modal to show up.
const WAIT_FOR_MODAL: Duration = Duration::from_secs(25);

fn td_pid() -> u32 {
    std::env::var("TD_PID")
        .expect("set TD_PID to a running TouchDesigner pid")
        .parse()
        .expect("TD_PID must be numeric")
}

/// Poll until the pid reports at least one popup.
fn await_popup(src: &tdmcp_dialogs::MacDialogSource, pid: u32) -> PopupInfo {
    let deadline = Instant::now() + WAIT_FOR_MODAL;
    while Instant::now() < deadline {
        let snap = src.snapshot(pid);
        if let Some(p) = snap.popups.first() {
            println!(
                "  detected popup id={} class={:?} kind={:?} severity={:?} title={:?}",
                p.id, p.class, p.kind, p.severity, p.title
            );
            return p.clone();
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("no popup appeared within {WAIT_FOR_MODAL:?} - open a modal in TD first");
}

#[test]
#[ignore = "live: requires an OPEN modal dialog in TouchDesigner"]
fn modal_round_trip_list_describe_dismiss() {
    let pid = td_pid();
    let src = tdmcp_dialogs::MacDialogSource::new();

    // --- list -------------------------------------------------------------
    let popup = await_popup(&src, pid);
    let snap = src.snapshot(pid);
    assert_eq!(
        snap.window_status,
        Some(tdmcp_core::WindowStatus::BlockedByModalWindow),
        "an open modal must report blocked_by_modal_window"
    );
    assert!(
        !popup.is_main_chrome,
        "the modal must not be flagged as protected main chrome"
    );

    // --- describe ---------------------------------------------------------
    let full = src.describe(pid, &popup.id).expect("describe failed");
    println!("  message = {:?}", full.message);
    for b in &full.buttons {
        println!(
            "  button id={} label={:?} default={}",
            b.id, b.label, b.is_default
        );
    }
    assert!(
        !full.buttons.is_empty(),
        "describe returned zero buttons - the AX role normalization regressed"
    );
    assert!(
        full.message.as_deref().is_some_and(|m| !m.is_empty()),
        "describe returned no message text - the AX role normalization regressed"
    );

    // --- dismiss via an EXPLICIT button ----------------------------------
    // Picking a non-default button proves plan_ladder took the Click branch;
    // the old code always fell through to Close and ignored this argument.
    let target = full
        .buttons
        .iter()
        .find(|b| !b.is_default)
        .or_else(|| full.buttons.first())
        .expect("at least one button");
    let label = target.label.clone();
    println!("  dismissing via explicit button {label:?}");

    let outcome = src
        .dismiss(pid, &popup.id, Some(&label))
        .expect("dismiss failed");
    println!(
        "  outcome dismissed={} via={:?}",
        outcome.dismissed, outcome.via
    );
    assert!(outcome.dismissed, "dismiss reported failure");
    assert!(
        outcome.still_open.is_empty(),
        "verify-gone left {:?} open",
        outcome.still_open
    );
    assert_eq!(
        outcome.via.as_deref(),
        Some(format!("button:{label}").as_str()),
        "dismiss did not go through the explicit-button branch"
    );

    // --- verify the pid is clean again ------------------------------------
    let after = src.snapshot(pid);
    assert!(
        after.popups.iter().all(|p| p.id != popup.id),
        "popup {} still present after dismiss",
        popup.id
    );
    println!("  after dismiss: windowStatus={:?}", after.window_status);
}

/// Main chrome must be refused even if an id for it is passed explicitly.
#[test]
#[ignore = "live: requires a running TouchDesigner"]
fn main_chrome_is_refused_by_dismiss() {
    let pid = td_pid();
    let src = tdmcp_dialogs::MacDialogSource::new();
    let main = tdmcp_dialogs::sys::macos::top_level_windows(pid)
        .expect("enumerate")
        .into_iter()
        .find(|w| w.is_dialog == Some(false))
        .expect("no standard window found");
    // The editor window is filtered before it can become a popup, so dismiss
    // cannot resolve it at all - NotFound is the correct refusal here.
    let err = src.dismiss(pid, &main.id, None).unwrap_err();
    println!("  dismiss(main chrome) -> {err:?}");
    assert!(
        matches!(err, tdmcp_core::DialogError::NotFound { .. }),
        "expected the editor window to be unreachable as a popup, got {err:?}"
    );
}
