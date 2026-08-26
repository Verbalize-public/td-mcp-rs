//! td-mcp-rs tray dashboard (egui 0.35 + tray-icon + notify-rust).
//!
//! Consumed in-process by `tdmcp-daemon` when the `gui` feature is enabled.
//! Closing the window or losing focus only hides the UI — it does not stop
//! the daemon. Use Stop / `/admin/shutdown` to end the process (sets `quit`).
//!
//! Module map: [`app`] shared state/logic · [`tray`] status item · [`popup`]
//! glance card · [`dashboard`] secondary viewport · [`federation`] flows ·
//! [`wire`] admin DTOs · [`http`] blocking transport · [`platform`] OS shims ·
//! [`theme`] design tokens + widget kit.

mod app;
mod dashboard;
mod federation;
mod http;
mod platform;
mod popup;
mod theme;
mod tray;
mod wire;

// Dev-only fixture harness (see docs/GUI_OVERHAUL_PLAN.md §11).
#[cfg(feature = "preview")]
pub mod preview;

pub use platform::toast;

use eframe::egui;

use app::DashboardApp;
use tray::load_rgba;

/// Run the tray dashboard on the calling thread (must be the process main thread).
///
/// Polls `admin_base` (e.g. `http://127.0.0.1:9860`) for status/fleet/sessions.
/// `data_dir` is the install root (contains `bootstrap.tox`) for the reveal button.
/// `config_path` is the TOML settings file edited by the Settings view.
/// When `quit` is set (Stop / idle / admin shutdown), the event loop closes for real.
pub fn run(
    admin_base: String,
    data_dir: std::path::PathBuf,
    quit: std::sync::Arc<std::sync::atomic::AtomicBool>,
    config_path: std::path::PathBuf,
) -> anyhow::Result<()> {
    let icon_normal_full = load_rgba(include_bytes!("../assets/icon-normal.png"), None)?;
    let icon_normal = load_rgba(include_bytes!("../assets/icon-normal.png"), Some(32))?;
    let icon_attention = load_rgba(include_bytes!("../assets/icon-attention.png"), Some(32))?;
    let window_icon = egui::IconData {
        rgba: icon_normal_full.rgba.clone(),
        width: icon_normal_full.width,
        height: icon_normal_full.height,
    };
    let dash_icon = egui::IconData {
        rgba: window_icon.rgba.clone(),
        width: window_icon.width,
        height: window_icon.height,
    };

    #[allow(unused_mut)]
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([theme::WINDOW_WIDTH, 304.0])
            .with_min_inner_size([theme::WINDOW_WIDTH, 180.0])
            .with_max_inner_size([theme::WINDOW_WIDTH, theme::WINDOW_MAX_HEIGHT])
            .with_title("td-mcp-rs")
            .with_icon(window_icon)
            .with_decorations(false)
            .with_taskbar(false)
            .with_resizable(false)
            // Tray + toast only at startup; user opens the dashboard via the tray.
            .with_visible(false),
        ..Default::default()
    };
    #[cfg(target_os = "macos")]
    {
        // Menu-bar-only process: Accessory keeps us out of the Dock and is the
        // policy tray-icon / NSStatusItem expect for a persistent status item.
        options.event_loop_builder = Some(Box::new(|builder| {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            builder.with_activation_policy(ActivationPolicy::Accessory);
        }));
    }
    eframe::run_native(
        "td-mcp-rs",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(DashboardApp::new(
                admin_base,
                data_dir,
                icon_normal,
                icon_attention,
                quit,
                config_path,
                dash_icon,
            )?))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))
}
