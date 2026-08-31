//! Dev-only dashboard preview runner (feature `preview`).
//!
//! ```text
//! cargo run -p tdmcp-gui --features preview --example dashboard_preview
//! TDMCP_PREVIEW_SCENE=modal-add-slave cargo run -p tdmcp-gui --features preview --example dashboard_preview
//! ```
//!
//! Scenes: overview-empty · overview-populated · overview-offline ·
//! overview-narrow (800px min width) · overview-many (roster cap) ·
//! modal-add-slave · stop-confirm · logs-filtered · settings-dirty ·
//! popup · popup-stop-confirm (tray glance card at its real size).

fn main() {
    let scene =
        std::env::var("TDMCP_PREVIEW_SCENE").unwrap_or_else(|_| "overview-populated".to_owned());
    if let Err(e) = tdmcp_gui::preview::run(&scene) {
        eprintln!("preview failed: {e}");
        std::process::exit(1);
    }
}
