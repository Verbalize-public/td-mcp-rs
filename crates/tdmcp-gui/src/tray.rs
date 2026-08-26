//! Tray status item: icon assets, deferred build, click handling, and
//! popup positioning near the tray anchor.

use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui;
use tracing::{info, warn};
use tray_icon::{Icon, MouseButton, MouseButtonState, Rect, TrayIconBuilder, TrayIconEvent};

use crate::app::DashboardApp;
use crate::theme::WINDOW_WIDTH;

/// Coalesce tray click bursts so double events cannot flip twice.
const TRAY_TOGGLE_DEBOUNCE: Duration = Duration::from_millis(250);

pub(crate) struct RgbaIcon {
    pub(crate) rgba: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

pub(crate) fn load_rgba(bytes: &[u8], max_side: Option<u32>) -> Result<RgbaIcon> {
    let img = image::load_from_memory(bytes)?.into_rgba8();
    let (width, height) = img.dimensions();
    let resized = match max_side {
        Some(side) if width > side || height > side => {
            image::imageops::resize(&img, side, side, image::imageops::FilterType::Lanczos3)
        }
        _ => {
            if width > 256 || height > 256 {
                image::imageops::resize(&img, 256, 256, image::imageops::FilterType::Lanczos3)
            } else {
                img
            }
        }
    };
    let (width, height) = resized.dimensions();
    Ok(RgbaIcon {
        rgba: resized.into_raw(),
        width,
        height,
    })
}

pub(crate) fn tray_icon_from(rgba: &RgbaIcon) -> Result<Icon> {
    Icon::from_rgba(rgba.rgba.clone(), rgba.width, rgba.height)
        .map_err(|e| anyhow::anyhow!("tray icon: {e}"))
}

impl DashboardApp {
    pub(crate) fn ensure_tray(&mut self) {
        if !self.pending_tray || self.tray.is_some() {
            return;
        }
        self.pending_tray = false;
        // No context menu: left click = dashboard, right click = glance panel
        // (see `handle_tray_events`). Daemon Stop/Restart live in the
        // dashboard's DAEMON card.
        let icon = match tray_icon_from(&self.icon_normal) {
            Ok(i) => i,
            Err(e) => {
                warn!(error = %e, "tray icon decode failed");
                return;
            }
        };
        match TrayIconBuilder::new()
            .with_tooltip("td-mcp-rs")
            .with_icon(icon)
            // Do not set template mode: our PNGs are full-color opaque RGB.
            // macOS template icons need black+alpha shapes; template+opaque
            // color assets often render as an invisible menu-bar item.
            .build()
        {
            Ok(tray) => {
                info!("tray status item created");
                self.tray = Some(tray);
            }
            Err(e) => warn!(error = %e, "tray icon build failed"),
        }
    }

    fn tray_click_debounced(&mut self) -> bool {
        let now = Instant::now();
        if self
            .last_tray_toggle_at
            .is_some_and(|t| now.duration_since(t) < TRAY_TOGGLE_DEBOUNCE)
        {
            return false;
        }
        self.last_tray_toggle_at = Some(now);
        true
    }

    /// Tray right-click Down: hide the glance panel when open. Hiding on Down
    /// (not Up) avoids a focus-loss → Up reopen blink.
    fn on_tray_popup_down(&mut self, ctx: &egui::Context) {
        if self.visible {
            self.hide_window(ctx);
            self.tray_popup_close_on_up = true;
        } else {
            self.tray_popup_close_on_up = false;
        }
    }

    /// Tray right-click Up: open the glance panel when closed (unless Down
    /// just closed it for this gesture).
    fn on_tray_popup_up(&mut self, ctx: &egui::Context, tray_rect: Rect) {
        if !self.tray_click_debounced() {
            return;
        }
        self.last_tray_rect = Some(tray_rect);

        if !self.visible && !self.tray_popup_close_on_up {
            self.show_window(ctx, Some(tray_rect));
        }
        self.tray_popup_close_on_up = false;
    }

    pub(crate) fn handle_tray_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            match event {
                // Left click opens/focuses the dashboard; DoubleClick ignored.
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    if self.tray_click_debounced() {
                        self.open_or_focus_dashboard(ctx);
                    }
                }
                // Right click toggles the glance panel near the tray.
                // Down hides when open; Up opens when closed — split so
                // focus-loss on Down cannot make Up reopen immediately.
                TrayIconEvent::Click {
                    button: MouseButton::Right,
                    button_state: MouseButtonState::Down,
                    ..
                } => {
                    self.on_tray_popup_down(ctx);
                }
                TrayIconEvent::Click {
                    button: MouseButton::Right,
                    button_state: MouseButtonState::Up,
                    rect,
                    ..
                } => {
                    self.on_tray_popup_up(ctx, rect);
                }
                _ => {}
            }
        }
    }

    pub(crate) fn position_near_tray(&self, ctx: &egui::Context) {
        let Some(rect) = self.last_tray_rect else {
            return;
        };

        // tray-icon reports the icon rect in *physical* pixels, but egui's
        // `OuterPosition` expects *logical* points. On HiDPI displays the raw
        // physical coords land the window off-screen (it still renders, so the
        // taskbar preview shows it, but it's invisible). Convert via the OS
        // scale, then clamp to the current monitor so it can never escape.
        let scale = ctx
            .input(|i| i.viewport().native_pixels_per_point)
            .unwrap_or(1.0) as f64;
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let icon_x = rect.position.x / scale;
        let icon_y = rect.position.y / scale;
        let icon_w = f64::from(rect.size.width) / scale;
        let icon_h = f64::from(rect.size.height) / scale;

        // Use the window's real size (logical points) so the popup is placed with
        // its true footprint and can never walk off the monitor via a stale height.
        let (popup_w, popup_h) = ctx
            .input(|i| i.viewport().outer_rect)
            .map(|r| (f64::from(r.width()), f64::from(r.height())))
            .filter(|(w, h)| *w > 0.0 && *h > 0.0)
            .unwrap_or((f64::from(WINDOW_WIDTH), 360.0));

        // Current monitor bounds in logical points. `monitor_size` is the size of
        // the monitor the window is on (the window was hidden near the tray last,
        // so this is normally the tray's monitor). Derive its origin by tiling the
        // tray position into the monitor grid — good enough for the common
        // same-sized-monitor case, and far better than landing off-screen.
        let mon = ctx
            .input(|i| i.viewport().monitor_size)
            .map(|m| (m.x as f64, m.y as f64));
        let (mon_x, mon_y, mon_w, mon_h) = match mon {
            Some((w, h)) if w > 0.0 && h > 0.0 => {
                let ox = (icon_x / w).floor() * w;
                let oy = (icon_y / h).floor() * h;
                (ox, oy, w, h)
            }
            _ => (0.0, 0.0, f64::INFINITY, f64::INFINITY),
        };

        // Bottom taskbar: grow up-left from icon. Top taskbar: grow down-left.
        // Flush to the dock/taskbar edge (no gap).
        let taskbar_bottom = icon_y > mon_y + mon_h / 2.0;
        let mut x = icon_x + icon_w - popup_w;
        let mut y = if taskbar_bottom {
            icon_y - popup_h
        } else {
            icon_y + icon_h
        };

        // Clamp so the whole popup stays on the monitor.
        x = x.max(mon_x).min(mon_x + mon_w - popup_w);
        y = y.max(mon_y).min(mon_y + mon_h - popup_h);

        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
            x as f32, y as f32,
        )));
    }
}
