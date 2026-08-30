//! Self-contained control test for the AX content path.
//!
//! Spawns a real, AX-exposing macOS dialog (`osascript display dialog`) and
//! drives the full list → describe → dismiss round trip against it. Unlike the
//! TouchDesigner tests this needs no TD, and unlike TD's own dialogs this one
//! publishes real buttons and labels to the accessibility tree — so it is the
//! test that actually proves `describe` reads labels and that `dismiss` takes
//! the explicit-button branch rather than falling through to `Close`.
//!
//! ```text
//! cargo test -p tdmcp-dialogs --test live_native_dialog -- --ignored --nocapture
//! ```
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "live tests")]

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use tdmcp_core::DialogSource;

/// Guard so the dialog process never outlives a failing test.
struct Dialog(Child);

impl Drop for Dialog {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_dialog() -> Dialog {
    let child = Command::new("osascript")
        .arg("-e")
        .arg(
            r#"display dialog "tdmcp control probe body" buttons {"Cancel", "OK"} default button "OK""#,
        )
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn osascript");
    Dialog(child)
}

#[test]
#[ignore = "live: opens a real GUI dialog; requires TCC Accessibility"]
fn native_dialog_round_trip_reads_labels_and_honors_explicit_button() {
    let dialog = spawn_dialog();
    let pid = dialog.0.id();
    let src = tdmcp_dialogs::MacDialogSource::new();

    // --- list: wait for the dialog to be up ------------------------------
    let deadline = Instant::now() + Duration::from_secs(15);
    let popup = loop {
        assert!(Instant::now() < deadline, "dialog never appeared");
        if let Some(p) = src.snapshot(pid).popups.first().cloned() {
            break p;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    println!(
        "  popup id={} class={:?} kind={:?}",
        popup.id, popup.class, popup.kind
    );
    assert_eq!(popup.class.as_deref(), Some("AXDialog"));
    assert!(!popup.is_main_chrome);

    // --- describe: labels must actually come through ----------------------
    let full = src.describe(pid, &popup.id).expect("describe");
    println!("  message = {:?}", full.message);
    for b in &full.buttons {
        println!(
            "  button id={} label={:?} default={}",
            b.id, b.label, b.is_default
        );
    }
    let labels: Vec<&str> = full.buttons.iter().map(|b| b.label.as_str()).collect();
    assert!(
        labels.contains(&"OK") && labels.contains(&"Cancel"),
        "expected OK and Cancel, got {labels:?}"
    );
    assert!(
        full.buttons.iter().all(|b| !b.label.is_empty()),
        "a button came back with an empty label: {labels:?}"
    );
    assert!(
        full.buttons.iter().any(|b| b.is_default && b.label == "OK"),
        "AXDefaultButton did not mark OK as the default: {:?}",
        full.buttons
    );
    assert!(
        full.message
            .as_deref()
            .is_some_and(|m| m.contains("control probe")),
        "message text missing: {:?}",
        full.message
    );

    // --- dismiss: explicitly press the NON-default button -----------------
    // If plan_ladder ignored `button` it would press OK (the default) or fall
    // through to Close; `via` proves which branch actually ran.
    let outcome = src
        .dismiss(pid, &popup.id, Some("Cancel"))
        .expect("dismiss");
    println!(
        "  outcome dismissed={} via={:?}",
        outcome.dismissed, outcome.via
    );
    assert!(outcome.dismissed);
    assert_eq!(
        outcome.via.as_deref(),
        Some("button:Cancel"),
        "dismiss did not take the explicit-button branch"
    );
    assert!(outcome.still_open.is_empty());
    assert!(
        src.snapshot(pid).popups.is_empty(),
        "dialog still present after dismiss"
    );
}
