//! Tray status item: icon assets, deferred build, click handling, and
//! popup positioning near the tray anchor.
//!
//! Two backends behind one [`TrayHandle`]: Windows/macOS keep `tray-icon`
//! (native status item); Linux uses `ksni` (pure-Rust StatusNotifierItem
//! over DBus) because tray-icon's Linux backend needs GTK + libappindicator,
//! whose absence crashed the whole daemon (L-10). Gesture state and popup
//! positioning are backend-agnostic; only build/teardown and event intake
//! differ.

use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui;
use tracing::{info, warn};

#[cfg(not(target_os = "linux"))]
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
#[cfg(not(target_os = "linux"))]
use tray_icon::{Icon, MouseButton, MouseButtonState, Rect, TrayIconBuilder, TrayIconEvent};

#[cfg(target_os = "linux")]
use ksni::blocking::Handle as KsniHandle;

use crate::app::DashboardApp;
use crate::theme::WINDOW_WIDTH;

/// Coalesce tray click bursts so double events cannot flip twice.
const TRAY_TOGGLE_DEBOUNCE: Duration = Duration::from_millis(250);
/// Hold a single left click this long before opening the glance popup, so the
/// second half of a double click can claim the gesture for the dashboard
/// instead. Windows' own double-click time defaults to 500ms; a shorter grace
/// keeps the popup snappy, and a late DoubleClick still hides the popup and
/// opens the dashboard.
const TRAY_DOUBLE_CLICK_GRACE: Duration = Duration::from_millis(300);

/// Context-menu item ids (`MenuEvent` carries only the id). ksni menus carry
/// typed events instead, so these exist only for the native backend.
#[cfg(not(target_os = "linux"))]
const MENU_DASHBOARD: &str = "tray.dashboard";
#[cfg(not(target_os = "linux"))]
const MENU_STOP: &str = "tray.stop";
#[cfg(not(target_os = "linux"))]
const MENU_RESTART: &str = "tray.restart";

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

#[cfg(not(target_os = "linux"))]
pub(crate) fn tray_icon_from(rgba: &RgbaIcon) -> Result<Icon> {
    Icon::from_rgba(rgba.rgba.clone(), rgba.width, rgba.height)
        .map_err(|e| anyhow::anyhow!("tray icon: {e}"))
}

/// Crate-local tray anchor rect. `tray_icon::Rect` leaks its dpi types into
/// `app.rs`; on Linux ksni only hands over an activate point, so both
/// backends normalize into these plain `f64` fields (physical pixels).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct TrayRect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

#[cfg(not(target_os = "linux"))]
impl From<Rect> for TrayRect {
    fn from(rect: Rect) -> Self {
        TrayRect {
            x: rect.position.x,
            y: rect.position.y,
            width: f64::from(rect.size.width),
            height: f64::from(rect.size.height),
        }
    }
}

/// Pull a readable string out of a panic payload (best-effort; payloads may
/// be `&str`, `String`, or anything else). Only the native backend needs it.
#[cfg(not(target_os = "linux"))]
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_owned())
}

/// Platform tray handle: the native item (tray-icon) or the ksni DBus handle.
/// The GUI only calls the two setters, so status updates stay
/// backend-agnostic.
pub(crate) enum TrayHandle {
    #[cfg(not(target_os = "linux"))]
    TrayIcon(tray_icon::TrayIcon),
    #[cfg(target_os = "linux")]
    Ksni(KsniHandle<KsniTrayState>),
}

impl TrayHandle {
    /// Push the fleet status line to the tray tooltip.
    pub(crate) fn set_tooltip(&self, text: &str) {
        match self {
            #[cfg(not(target_os = "linux"))]
            TrayHandle::TrayIcon(t) => {
                let _ = t.set_tooltip(Some(text));
            }
            #[cfg(target_os = "linux")]
            TrayHandle::Ksni(h) => {
                // `None` = the ksni service is gone; keep the dashboard, drop
                // only the tooltip.
                let _ = h.update(|s| s.tooltip = text.to_owned());
            }
        }
    }

    /// Swap the tray icon (normal ↔ attention).
    pub(crate) fn set_icon(&self, icon: &RgbaIcon) {
        match self {
            #[cfg(not(target_os = "linux"))]
            TrayHandle::TrayIcon(t) => {
                if let Ok(ti) = tray_icon_from(icon) {
                    let _ = t.set_icon(Some(ti));
                }
            }
            #[cfg(target_os = "linux")]
            TrayHandle::Ksni(h) => {
                let data = argb_from_rgba(&icon.rgba);
                let width = icon.width as i32;
                let height = icon.height as i32;
                let _ = h.update(move |s| {
                    s.icon = ksni::Icon {
                        width,
                        height,
                        data,
                    };
                });
            }
        }
    }
}

// The shutdown request is just a channel send; the awaiter may be dropped
// unawaited and the ksni service thread exits on its own.
#[cfg(target_os = "linux")]
impl Drop for TrayHandle {
    fn drop(&mut self) {
        let TrayHandle::Ksni(h) = self;
        h.shutdown();
    }
}

impl DashboardApp {
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

    /// Tray left-click Down: hide the glance panel when open. Hiding on Down
    /// (not Up) avoids a focus-loss → Up reopen blink.
    fn on_tray_popup_down(&mut self, ctx: &egui::Context) {
        if self.visible {
            self.hide_window(ctx);
            self.tray_popup_close_on_up = true;
        } else {
            self.tray_popup_close_on_up = false;
        }
        self.tray_popup_open_at = None;
    }

    /// Tray left-click Up: arm the glance panel to open after the double-click
    /// grace (unless Down just closed it for this gesture).
    fn on_tray_popup_up(&mut self, ctx: &egui::Context, tray_rect: TrayRect) {
        if !self.tray_click_debounced() {
            return;
        }
        self.last_tray_rect = Some(tray_rect);

        if !self.visible && !self.tray_popup_close_on_up && !self.recent_outside_click_close() {
            let at = Instant::now() + TRAY_DOUBLE_CLICK_GRACE;
            self.tray_popup_open_at = Some(at);
            ctx.request_repaint_after(TRAY_DOUBLE_CLICK_GRACE);
        }
        self.tray_popup_close_on_up = false;
    }

    /// Tray left double-click: cancel the armed popup and open the dashboard.
    fn on_tray_double_click(&mut self, ctx: &egui::Context) {
        self.tray_popup_open_at = None;
        self.tray_swallow_left_up = true;
        if self.visible {
            self.hide_window(ctx);
        }
        self.open_or_focus_dashboard(ctx);
    }

    /// Open the glance popup once the double-click grace has elapsed with no
    /// DoubleClick. Called every tick from the app loop.
    pub(crate) fn flush_pending_tray_popup(&mut self, ctx: &egui::Context) {
        if self
            .tray_popup_open_at
            .is_some_and(|at| Instant::now() >= at)
        {
            self.tray_popup_open_at = None;
            if !self.visible {
                self.show_window(ctx, None);
            }
        }
    }

    pub(crate) fn position_near_tray(&self, ctx: &egui::Context) {
        let Some(rect) = self.last_tray_rect else {
            return;
        };

        // The tray anchor arrives in *physical* pixels, but egui's
        // `OuterPosition` expects *logical* points. On HiDPI displays the raw
        // physical coords land the window off-screen (it still renders, so the
        // taskbar preview shows it, but it's invisible). Convert via the OS
        // scale, then clamp to the current monitor so it can never escape.
        let scale = ctx
            .input(|i| i.viewport().native_pixels_per_point)
            .unwrap_or(1.0) as f64;
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let icon_x = rect.x / scale;
        let icon_y = rect.y / scale;
        let icon_w = rect.width / scale;
        let icon_h = rect.height / scale;

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

// ------------------------------------------------------------------- //
// Windows/macOS: tray-icon native status item (behavior unchanged).   //
// ------------------------------------------------------------------- //
#[cfg(not(target_os = "linux"))]
impl DashboardApp {
    pub(crate) fn ensure_tray(&mut self) {
        if !self.pending_tray || self.tray.is_some() {
            return;
        }
        self.pending_tray = false;
        // Right click = context menu (below); left click = glance popup,
        // double click = dashboard (see `handle_tray_events`).
        let menu = Menu::new();
        let items = [
            MenuItem::with_id(MENU_DASHBOARD, "Dashboard", true, None),
            MenuItem::with_id(MENU_STOP, "Stop", true, None),
            MenuItem::with_id(MENU_RESTART, "Restart", true, None),
        ];
        for item in &items {
            if let Err(e) = menu.append(item) {
                warn!(error = %e, "tray menu append failed");
            }
        }
        let icon = match tray_icon_from(&self.icon_normal) {
            Ok(i) => i,
            Err(e) => {
                warn!(error = %e, "tray icon decode failed");
                return;
            }
        };
        // tray-icon fronts native APIs that can panic on unexpected OS state;
        // the status item is best-effort — lose the tray, keep the dashboard
        // (L-10).
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            TrayIconBuilder::new()
                .with_tooltip("td-mcp-rs")
                .with_icon(icon)
                .with_menu(Box::new(menu))
                // Left click stays ours (popup / dashboard); the menu is
                // right-click only.
                .with_menu_on_left_click(false)
                // Do not set template mode: our PNGs are full-color opaque RGB.
                // macOS template icons need black+alpha shapes; template+opaque
                // color assets often render as an invisible menu-bar item.
                .build()
        }));
        let tray = match built {
            Ok(Ok(tray)) => tray,
            Ok(Err(e)) => {
                warn!(error = %e, "tray icon build failed");
                return;
            }
            Err(payload) => {
                warn!(reason = %panic_message(payload.as_ref()), "tray icon build panicked");
                return;
            }
        };
        info!("tray status item created");
        self.tray = Some(TrayHandle::TrayIcon(tray));
    }

    pub(crate) fn handle_tray_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            match event {
                // Left click toggles the glance panel near the tray. Down hides
                // when open; Up arms the open — split so focus-loss on Down
                // cannot make Up reopen immediately.
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Down,
                    ..
                } => {
                    self.on_tray_popup_down(ctx);
                }
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    rect,
                    ..
                } => {
                    // Windows sends Down, Up, DoubleClick, Up for a double
                    // click — the trailing Up must not re-arm the popup.
                    if std::mem::take(&mut self.tray_swallow_left_up) {
                        continue;
                    }
                    self.on_tray_popup_up(ctx, TrayRect::from(rect));
                }
                // Double click opens the full dashboard and cancels the popup.
                // macOS never emits DoubleClick for status items; there the
                // popup's ⛶ button (or the menu) is the way in.
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => {
                    self.on_tray_double_click(ctx);
                }
                _ => {}
            }
        }

        // Right click shows the context menu (tray-icon handles the popup
        // itself); we only act on the chosen item.
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            match event.id.0.as_str() {
                MENU_DASHBOARD => self.open_or_focus_dashboard(ctx),
                MENU_STOP => self.shutdown_daemon(),
                MENU_RESTART => self.restart_daemon(),
                other => warn!(id = other, "unknown tray menu id"),
            }
        }
    }
}

// ------------------------------------------------------------------- //
// Linux: ksni StatusNotifierItem over DBus (no GTK, no libappindicator).
// ------------------------------------------------------------------- //

/// Events the ksni service forwards to the egui thread: menu choices and
/// left-click activations.
#[cfg(target_os = "linux")]
#[derive(Clone)]
pub(crate) enum TrayEvent {
    /// Left click on the item; x/y are screen pixels (positioning hint).
    Activate {
        x: f64,
        y: f64,
    },
    Dashboard,
    Stop,
    Restart,
}

/// ksni state owned by the SNI service loop; the egui thread writes it via
/// `Handle::update` and never touches it otherwise. `pub(crate)` only because
/// it appears inside the `pub(crate)` spawn/field types.
#[cfg(target_os = "linux")]
pub(crate) struct KsniTrayState {
    icon: ksni::Icon,
    tooltip: String,
    events: std::sync::mpsc::Sender<TrayEvent>,
}

#[cfg(target_os = "linux")]
impl ksni::Tray for KsniTrayState {
    fn id(&self) -> String {
        "tdmcp-rs".to_owned()
    }

    fn title(&self) -> String {
        "td-mcp-rs".to_owned()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        // The status line is the tooltip title, mirroring tray-icon's
        // single-string tooltip on Windows/macOS.
        ksni::ToolTip {
            title: self.tooltip.clone(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::menu::MenuItem<Self>> {
        // Send-and-return: the egui thread owns the receiver and applies the
        // same handlers as the native menus (`handle_tray_events`).
        let item = |label: &str, event: TrayEvent| {
            ksni::menu::StandardItem {
                label: label.to_owned(),
                activate: Box::new({
                    let tx = self.events.clone();
                    move |_state| {
                        let _ = tx.send(event.clone());
                    }
                }),
                ..Default::default()
            }
            .into()
        };
        vec![
            item("Dashboard", TrayEvent::Dashboard),
            item("Stop", TrayEvent::Stop),
            item("Restart", TrayEvent::Restart),
        ]
    }

    fn activate(&mut self, x: i32, y: i32) {
        // One event per click; the GUI synthesizes the Down→Up pair the
        // debouncing gesture state machine was built for.
        let _ = self.events.send(TrayEvent::Activate {
            x: f64::from(x),
            y: f64::from(y),
        });
    }
}

/// RGBA8 → ARGB32 (network byte order), the ksni reference conversion:
/// rotate each 4-byte pixel `[r,g,b,a]` → `[a,r,g,b]`.
#[cfg(target_os = "linux")]
fn argb_from_rgba(rgba: &[u8]) -> Vec<u8> {
    let mut data = rgba.to_vec();
    for pixel in data.as_chunks_mut::<4>().0 {
        pixel.rotate_right(1);
    }
    data
}

#[cfg(target_os = "linux")]
fn argb_icon(icon: &RgbaIcon) -> ksni::Icon {
    ksni::Icon {
        width: icon.width as i32,
        height: icon.height as i32,
        data: argb_from_rgba(&icon.rgba),
    }
}

#[cfg(target_os = "linux")]
pub(crate) type TraySpawnRx = std::sync::mpsc::Receiver<
    anyhow::Result<(
        KsniHandle<KsniTrayState>,
        std::sync::mpsc::Receiver<TrayEvent>,
    )>,
>;

/// Start the ksni SNI service off-thread. The returned channel yields
/// `Ok((handle, events))` once the session bus accepted the item, or the
/// reason it will not. The GUI thread only polls this — it never waits on
/// DBus, so a hung or missing session bus cannot stall the dashboard.
#[cfg(target_os = "linux")]
pub(crate) fn ksni_spawn(icon: ksni::Icon) -> TraySpawnRx {
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let (events_tx, events_rx) = std::sync::mpsc::channel::<TrayEvent>();
    let state = KsniTrayState {
        icon,
        tooltip: "td-mcp-rs".to_owned(),
        events: events_tx,
    };
    let _ = std::thread::Builder::new()
        .name("tdmcp-tray".to_owned())
        .spawn(move || {
            use ksni::blocking::TrayMethods;
            // `spawn` connects + registers synchronously, then runs the
            // service loop on a thread of its own.
            let _ = result_tx.send(
                state
                    .spawn()
                    .map(|handle| (handle, events_rx))
                    .map_err(Into::into),
            );
        });
    result_rx
}

#[cfg(target_os = "linux")]
impl DashboardApp {
    pub(crate) fn ensure_tray(&mut self) {
        if !self.pending_tray || self.tray.is_some() {
            return;
        }
        self.pending_tray = false;
        self.tray_spawn = Some(ksni_spawn(argb_icon(&self.icon_normal)));
    }

    pub(crate) fn handle_tray_events(&mut self, ctx: &egui::Context) {
        // Pick up the spawn result; the GUI thread never waits on DBus.
        if let Some(rx) = self.tray_spawn.take() {
            match rx.try_recv() {
                Ok(Ok((handle, events))) => {
                    self.tray = Some(TrayHandle::Ksni(handle));
                    self.tray_events = Some(events);
                    info!("tray status item created");
                }
                Ok(Err(e)) => {
                    warn!(error = %e, "tray status item unavailable");
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    self.tray_spawn = Some(rx);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // The spawn thread died mid-connect — degrade to no tray.
                    warn!("tray spawn thread exited without a result");
                }
            }
        }
        let Some(events) = self.tray_events.take() else {
            return;
        };
        while let Ok(event) = events.try_recv() {
            match event {
                TrayEvent::Activate { x, y } => {
                    // Two activations inside the grace are the halves of a
                    // double click: the second claims the dashboard, exactly
                    // like the native DoubleClick event.
                    if self
                        .last_tray_toggle_at
                        .is_some_and(|t| Instant::now().duration_since(t) < TRAY_DOUBLE_CLICK_GRACE)
                    {
                        self.on_tray_double_click(ctx);
                    } else {
                        // One activation per click: run the Down→Up pair the
                        // debouncing gestures expect (Down first so an open
                        // popup hides before the Up decides to re-arm).
                        self.on_tray_popup_down(ctx);
                        self.on_tray_popup_up(
                            ctx,
                            TrayRect {
                                x,
                                y,
                                width: 0.0,
                                height: 0.0,
                            },
                        );
                    }
                }
                TrayEvent::Dashboard => self.open_or_focus_dashboard(ctx),
                TrayEvent::Stop => self.shutdown_daemon(),
                TrayEvent::Restart => self.restart_daemon(),
            }
        }
        self.tray_events = Some(events);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use super::*;
    use crate::app::DashboardApp;

    fn app() -> DashboardApp {
        let tmp = std::env::temp_dir();
        let icon = || RgbaIcon {
            rgba: vec![0, 0, 0, 255],
            width: 1,
            height: 1,
        };
        let mut app = DashboardApp::new(
            "http://127.0.0.1:9860".to_owned(),
            tmp.clone(),
            icon(),
            icon(),
            Arc::new(AtomicBool::new(false)),
            tmp.join("tdmcp-tray-test-config.toml"),
            egui::IconData {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            },
        )
        .expect("build app");
        app.pending_tray = false;
        app.pending_initial_hide = false;
        app
    }

    fn rect() -> TrayRect {
        TrayRect::default()
    }

    /// Force the armed popup due without sleeping through the grace.
    fn expire_grace(app: &mut DashboardApp) {
        if app.tray_popup_open_at.is_some() {
            app.tray_popup_open_at = Some(Instant::now());
        }
    }

    #[test]
    fn single_left_click_opens_popup_after_grace() {
        let ctx = egui::Context::default();
        let mut app = app();

        app.on_tray_popup_down(&ctx);
        app.on_tray_popup_up(&ctx, rect());
        // Armed, not open — the double-click grace has not elapsed.
        assert!(app.tray_popup_open_at.is_some());
        assert!(!app.visible);

        expire_grace(&mut app);
        app.flush_pending_tray_popup(&ctx);
        assert!(app.visible);
        assert!(!app.dashboard_open);
    }

    #[test]
    fn double_click_opens_dashboard_and_never_the_popup() {
        let ctx = egui::Context::default();
        let mut app = app();

        // Windows order: Down, Up, DoubleClick, Up.
        app.on_tray_popup_down(&ctx);
        app.on_tray_popup_up(&ctx, rect());
        app.on_tray_double_click(&ctx);
        assert!(
            std::mem::take(&mut app.tray_swallow_left_up),
            "trailing Up is swallowed"
        );

        expire_grace(&mut app);
        app.flush_pending_tray_popup(&ctx);
        assert!(!app.visible, "popup must not open behind the dashboard");
        assert!(app.dashboard_open);
    }

    #[test]
    fn left_click_on_open_popup_closes_without_rearming() {
        let ctx = egui::Context::default();
        let mut app = app();
        app.visible = true;

        app.on_tray_popup_down(&ctx);
        assert!(!app.visible);
        app.on_tray_popup_up(&ctx, rect());

        expire_grace(&mut app);
        app.flush_pending_tray_popup(&ctx);
        assert!(!app.visible, "Up must not reopen what Down just closed");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ksni_activate_toggles_popup_and_doubles_into_dashboard() {
        let ctx = egui::Context::default();
        let mut app = app();

        // Single activate arms the popup (down+up pair).
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = tx.send(TrayEvent::Activate { x: 10.0, y: 20.0 });
        app.tray_events = Some(rx);
        app.handle_tray_events(&ctx);
        assert!(app.tray_popup_open_at.is_some(), "popup armed");
        assert_eq!(
            app.last_tray_rect,
            Some(TrayRect {
                x: 10.0,
                y: 20.0,
                width: 0.0,
                height: 0.0
            })
        );

        // Second activate inside the grace = double click → dashboard.
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = tx.send(TrayEvent::Activate { x: 10.0, y: 20.0 });
        app.tray_events = Some(rx);
        app.handle_tray_events(&ctx);
        assert!(app.tray_popup_open_at.is_none(), "armed popup cancelled");
        assert!(app.dashboard_open, "dashboard claimed by double click");
        assert!(!app.visible);

        expire_grace(&mut app);
        app.flush_pending_tray_popup(&ctx);
        assert!(!app.visible, "popup must not open behind the dashboard");
    }
}
