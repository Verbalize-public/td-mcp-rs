//! `DashboardApp` — shared state and logic core for both GUI surfaces
//! (tray popup + dashboard viewport): polling, settings editing, log
//! streaming state, window/tray lifecycle orchestration.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use eframe::egui;
use tdmcp_config::{self as cfgfile, ConfigFile, FIELD_DESCS};
use tracing::warn;
use uuid::Uuid;

use crate::dashboard;
use crate::http::{http_get_blocking, http_post_blocking};
use crate::platform::{notify, reveal_in_file_manager};
use crate::tray::{RgbaIcon, TrayHandle, TrayRect};
use crate::wire::{parse_slaves, FleetView, LogsResponse, StatusView};

/// Newest-first error/warning entries kept for popup + dashboard surfaces.
pub(crate) const ERROR_RING_CAP: usize = 50;
/// Rendered log rows kept client-side (evict oldest; matches the daemon ring cap).
const LOGS_RENDER_CAP: usize = 2048;
/// Records requested per `/admin/logs` fetch while the Logs view is active.
const LOGS_FETCH_LIMIT: u32 = 512;
/// Directory-listing cadence for crash reports (poll ticks at 2s; the scan
/// itself is a cheap readdir, so 5s is plenty fresh).
const CRASH_SCAN_INTERVAL: Duration = Duration::from_secs(5);

/// After an outside-click close, a tray Up within this window is the release
/// half of that same click — it must not re-open the popup.
const OUTSIDE_CLOSE_TOGGLE_GUARD: Duration = Duration::from_millis(700);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FleetPanel {
    None,
    AddSlave,
    SlaveSettings,
}

/// Transient bottom-right acknowledgment for async user actions.
#[derive(Debug, Clone)]
pub(crate) struct Snack {
    pub(crate) msg: String,
    pub(crate) tone: SnackTone,
    pub(crate) at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnackTone {
    Info,
    Ok,
    Warn,
    Error,
}

pub(crate) struct DashboardApp {
    pub(crate) admin_base: String,
    pub(crate) data_dir: PathBuf,
    pub(crate) config_path: PathBuf,
    pub(crate) draft: ConfigFile,
    pub(crate) settings_error: Option<String>,
    pub(crate) settings_save: crate::background::Background<(ConfigFile, bool)>,
    logs_fetch: crate::background::Background<String>,
    /// Text buffers for optional advanced paths (empty = unset).
    pub(crate) data_dir_edit: String,
    pub(crate) bridge_dir_edit: String,
    pub(crate) catalog_path_edit: String,
    pub(crate) daemon_bin_edit: String,
    pub(crate) template_path_edit: String,
    /// Text buffers for `[official_tools]` overrides (empty = unset).
    pub(crate) official_td_exe_edit: String,
    pub(crate) official_expand_edit: String,
    pub(crate) official_collapse_edit: String,
    pub(crate) official_wine_exe_edit: String,
    pub(crate) official_wine_prefix_edit: String,
    pub(crate) status: Option<StatusView>,
    pub(crate) fleet_json: String,
    pub(crate) sessions_json: String,
    /// Master-only: `/admin/federation/slaves` body.
    pub(crate) slaves_json: String,
    pub(crate) last_poll: Option<Instant>,
    poll_rx: Option<std::sync::mpsc::Receiver<crate::http::PollSnapshot>>,
    pub(crate) error: Option<String>,
    pub(crate) tray: Option<TrayHandle>,
    pub(crate) icon_normal: RgbaIcon,
    pub(crate) icon_attention: RgbaIcon,
    pub(crate) attention: bool,
    pub(crate) prev_snapshot: FleetSnapshot,
    pub(crate) visible: bool,
    /// Apply `Visible(false)` once after the first frame.
    pub(crate) pending_initial_hide: bool,
    /// Build the status-item on the first `logic` tick (see `ensure_tray`).
    pub(crate) pending_tray: bool,
    /// Fired once after the first successful `/admin/status` poll.
    pub(crate) startup_notified: bool,
    /// Fired once when polls fail before any success.
    pub(crate) startup_fail_notified: bool,
    pub(crate) fail_polls: u32,
    /// Drop always-on-top after this instant (transient focus grab).
    pub(crate) clear_always_on_top_at: Option<Instant>,
    /// Suppress focus-loss hide briefly after show (tray click focus race).
    pub(crate) ignore_focus_loss_until: Option<Instant>,
    /// Last tray toggle gesture (debounce burst events).
    pub(crate) last_tray_toggle_at: Option<Instant>,
    /// Left-click Down hid the popup — suppress Up from reopening (anti-blink).
    pub(crate) tray_popup_close_on_up: bool,
    /// A pending single left-click opens the glance popup at this instant,
    /// unless a DoubleClick lands first and claims the gesture.
    pub(crate) tray_popup_open_at: Option<Instant>,
    /// DoubleClick consumed the gesture — swallow the trailing Click Up.
    pub(crate) tray_swallow_left_up: bool,
    /// egui's view of the cursor: over our window (PointerMoved / press) or
    /// gone (PointerGone). Feeds the X11 outside-click poll.
    pub(crate) pointer_inside: bool,
    /// A press started inside the popup and no all-buttons-up poll has run
    /// since — suppresses outside-close while dragging out of the popup.
    pub(crate) press_inside_active: bool,
    /// Outside-click detection live (first all-buttons-up poll after show):
    /// keeps the very press that opened the popup from closing it.
    pub(crate) outside_click_armed: bool,
    /// Last outside-click close — the tray Up half of the same physical click
    /// must not reopen the popup.
    pub(crate) outside_click_close_at: Option<Instant>,
    /// X11 pointer poll connection for outside-click close (lazy).
    pub(crate) x11_pointer: Option<crate::platform::glance_pointer::PointerPoller>,
    /// Last tray icon rect for anchoring.
    pub(crate) last_tray_rect: Option<TrayRect>,
    /// Linux: pending ksni spawn result — the DBus connect runs off-thread
    /// and the GUI only polls it, so a missing/hung session bus never stalls
    /// the UI (L-10).
    #[cfg(target_os = "linux")]
    pub(crate) tray_spawn: Option<crate::tray::TraySpawnRx>,
    /// Linux: tray event stream (menu choices + left-click activations).
    #[cfg(target_os = "linux")]
    pub(crate) tray_events: Option<std::sync::mpsc::Receiver<crate::tray::TrayEvent>>,
    /// Shared with the daemon thread — when set, close the event loop for real.
    pub(crate) quit: Arc<AtomicBool>,
    /// Show auth PSK in Settings.
    pub(crate) show_psk: bool,
    /// Show master PSK in Settings.
    pub(crate) show_master_psk: bool,
    /// Confirm label after federation role change in Settings.
    pub(crate) role_change_note: Option<String>,
    /// Slave: awaiting confirm for Go standalone.
    pub(crate) confirm_go_standalone: bool,
    /// Federation overlay panel.
    pub(crate) fleet_panel: FleetPanel,
    pub(crate) add_slave_host: String,
    pub(crate) add_slave_port: u16,
    pub(crate) add_slave_psk: String,
    /// Outcome of the last one-click add-slave attempt.
    pub(crate) add_slave_step: crate::federation::AddSlaveStep,
    pub(crate) add_slave_job: crate::background::Background<crate::wire::FederationProbe>,
    pub(crate) add_slave_probe: Option<crate::wire::FederationProbe>,
    pub(crate) scan_results: Vec<crate::wire::ScanHit>,
    pub(crate) scan_busy: bool,
    /// Target slave for settings panel.
    pub(crate) slave_settings_target: Option<crate::wire::SlaveSettingsTarget>,
    pub(crate) slave_settings_call_timeout: u64,
    pub(crate) slave_settings_script_timeout: u64,
    pub(crate) slave_settings_error: Option<String>,
    pub(crate) show_add_slave_psk: bool,
    /// Pending scan result receiver (None when idle).
    pub(crate) scan_rx: Option<std::sync::mpsc::Receiver<Vec<crate::wire::ScanHit>>>,
    /// Slave self-view transient message (Go standalone outcome).
    pub(crate) slave_self_message: Option<String>,
    /// Which flow the current `scan_results` belong to.
    pub(crate) scan_purpose: crate::wire::ScanPurpose,
    /// Awaiting confirm for turning off network sharing while federated.
    pub(crate) confirm_turn_off_sharing: bool,
    /// One-shot: focus the Master PSK field next time it draws.
    pub(crate) focus_master_psk: bool,
    /// A saved setting needs a daemon restart to take effect.
    pub(crate) needs_restart: bool,
    /// Config snapshot as of the last `open_settings()`, for restart-needed diffing.
    pub(crate) settings_loaded_snapshot: ConfigFile,
    /// Slave daemon ids seen on the last `/admin/federation/slaves` poll.
    pub(crate) known_slave_ids: HashSet<String>,
    /// Suppresses a join-toast burst for slaves already connected at GUI start.
    pub(crate) slaves_seen_once: bool,
    /// Logs view state (T4.2).
    pub(crate) logs_view: LogsViewState,
    /// Dashboard secondary viewport is open.
    pub(crate) dashboard_open: bool,
    /// `dashboard_open` as of the previous tick (visibility-edge detection).
    pub(crate) dash_open_prev: bool,
    /// Selected dashboard tab.
    pub(crate) dash_tab: dashboard::DashTab,
    /// Latest errors/warnings, newest first (tray strip + dashboard card).
    pub(crate) error_ring: Vec<String>,
    /// Newest crash report in `{data_dir}/crash`, when that directory has any.
    pub(crate) last_crash: Option<PathBuf>,
    /// Crash-report count from the last scan.
    pub(crate) crash_count: usize,
    /// Crash row acknowledged for this session (click opens the report).
    pub(crate) crash_seen: bool,
    /// Throttle gate for the crash-directory scan (`None` = never scanned).
    pub(crate) crash_scan_at: Option<Instant>,
    /// Two-step guard for the destructive daemon Stop action.
    pub(crate) confirm_stop: bool,
    /// Bottom-right action acknowledgments (≤3, ~3s TTL, pruned per frame).
    pub(crate) snacks: Vec<Snack>,
    /// Window icon reused by the dashboard viewport builder.
    pub(crate) window_icon: egui::IconData,
    /// LRU recent projects (≤16, deduped) persisted beside `data_dir`.
    pub(crate) recent_projects: Vec<PathBuf>,
    /// True while a `spawn_td` is in flight (poll via `spawn_rx`).
    pub(crate) spawn_busy: bool,
    pub(crate) spawn_rx: Option<std::sync::mpsc::Receiver<Result<serde_json::Value, String>>>,
    /// Whether the Open-project dropdown menu is visible.
    pub(crate) show_recent_menu: bool,
    /// Anchor for the dropdown popup (screen pos).
    pub(crate) recent_menu_anchor: Option<egui::Pos2>,
    /// Palette section: roster snapshot, selection, thumbnails, jobs.
    pub(crate) palette: crate::palette::PaletteView,
}

/// Tray Logs view state (T4.2). Polling only runs while this view is the
/// active one, the window is visible, and `paused` is false.
pub(crate) struct LogsViewState {
    pub(crate) buf: std::collections::VecDeque<crate::wire::LogRecordView>,
    /// Cursor for the next `/admin/logs?after=` fetch.
    pub(crate) next: u64,
    /// Auto-scroll to the newest row (only when already at the bottom).
    pub(crate) follow: bool,
    pub(crate) paused: bool,
    /// `None` = ALL; cycles ALL -> WARN -> ERROR -> ALL via the filter chips.
    pub(crate) min_level: Option<&'static str>,
    /// Empty = ALL sources; otherwise only these `src` values are kept.
    pub(crate) srcs: HashSet<&'static str>,
    /// Client-side substring filter over msg/target (dashboard search box).
    pub(crate) text_filter: String,
    /// `seq` of the expanded row, if any.
    pub(crate) expanded: Option<u64>,
    pub(crate) fetch_error: Option<String>,
    /// Resolved `/admin/logs/path` directory (fetched lazily, once).
    pub(crate) dir: Option<String>,
    pub(crate) last_fetch: Option<Instant>,
}

impl Default for LogsViewState {
    fn default() -> Self {
        Self {
            buf: std::collections::VecDeque::new(),
            next: 0,
            follow: true,
            paused: false,
            min_level: None,
            srcs: HashSet::new(),
            text_filter: String::new(),
            expanded: None,
            fetch_error: None,
            dir: None,
            last_fetch: None,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub(crate) struct FleetSnapshot {
    pub(crate) connected: usize,
    pub(crate) disconnected: usize,
    pub(crate) resurrected: usize,
    pub(crate) cancelled: usize,
    pub(crate) connected_pids: Vec<u32>,
    pub(crate) resurrected_pids: Vec<u32>,
    pub(crate) cancelled_total: usize,
}

impl DashboardApp {
    pub(crate) fn new(
        admin_base: String,
        data_dir: PathBuf,
        icon_normal: RgbaIcon,
        icon_attention: RgbaIcon,
        quit: Arc<AtomicBool>,
        config_path: PathBuf,
        window_icon: egui::IconData,
    ) -> Result<Self> {
        let (draft, settings_error) = match cfgfile::load(&config_path) {
            Ok(config) => (config, None),
            Err(error) => (
                ConfigFile::default(),
                Some(format!("Cannot read configuration: {error}")),
            ),
        };
        // Dev/test hook: TDMCP_OPEN_DASH=1|logs|palette|settings opens the
        // dashboard (optionally on a tab) instead of staying tray-only.
        // `fleet` is kept as a back-compat alias for Overview since the tab merge.
        let dash_env = std::env::var("TDMCP_OPEN_DASH").unwrap_or_default();
        let dash_tab = match dash_env.as_str() {
            "logs" => dashboard::DashTab::Logs,
            "fleet" | "overview" => dashboard::DashTab::Overview,
            "palette" => dashboard::DashTab::Palette,
            "settings" => dashboard::DashTab::Settings,
            "federation" => dashboard::DashTab::Federation,
            _ => dashboard::DashTab::default(),
        };
        let dash_open = !dash_env.is_empty() && dash_env != "0";
        let edits = path_edits_from(&draft);
        let settings_loaded_snapshot = draft.clone();
        // Load recents from sidecar (outside config) — do after data_dir is known.
        // Use a temp data_dir for the load; the real one is `data_dir` param below
        // but we haven't moved it yet, so load after Self is built via helper.
        // Instead load inline here:
        let recent_projects = {
            let p = data_dir.join("recent_projects.json");
            std::fs::read_to_string(&p)
                .ok()
                .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
                .map(|v| v.into_iter().map(PathBuf::from).collect())
                .unwrap_or_default()
        };

        Ok(Self {
            admin_base,
            data_dir,
            config_path,
            draft,
            settings_error,
            data_dir_edit: edits.data_dir,
            bridge_dir_edit: edits.bridge_dir,
            catalog_path_edit: edits.catalog_path,
            daemon_bin_edit: edits.daemon_bin,
            template_path_edit: edits.template_path,
            official_td_exe_edit: edits.official_td_exe,
            official_expand_edit: edits.official_expand,
            official_collapse_edit: edits.official_collapse,
            official_wine_exe_edit: edits.official_wine_exe,
            official_wine_prefix_edit: edits.official_wine_prefix,
            status: None,
            fleet_json: String::new(),
            sessions_json: String::new(),
            slaves_json: String::new(),
            last_poll: None,
            poll_rx: None,
            settings_save: Default::default(),
            logs_fetch: Default::default(),
            error: None,
            // Defer tray build to the first `logic` tick. Creating a status-item
            // inside eframe's creation callback can re-enter AppKit on macOS and
            // trip winit 0.30's "event while another event is handled" abort.
            tray: None,
            icon_normal,
            icon_attention,
            attention: false,
            prev_snapshot: FleetSnapshot::default(),
            visible: false,
            pending_initial_hide: !dash_open,
            pending_tray: true,
            startup_notified: false,
            startup_fail_notified: false,
            fail_polls: 0,
            clear_always_on_top_at: None,
            ignore_focus_loss_until: None,
            last_tray_toggle_at: None,
            tray_popup_close_on_up: false,
            tray_popup_open_at: None,
            tray_swallow_left_up: false,
            pointer_inside: false,
            press_inside_active: false,
            outside_click_armed: false,
            outside_click_close_at: None,
            x11_pointer: None,
            last_tray_rect: None,
            #[cfg(target_os = "linux")]
            tray_spawn: None,
            #[cfg(target_os = "linux")]
            tray_events: None,
            quit,
            show_psk: false,
            show_master_psk: false,
            role_change_note: None,
            confirm_go_standalone: false,
            fleet_panel: FleetPanel::None,
            add_slave_host: String::new(),
            add_slave_port: tdmcp_config::DEFAULT_PORT,
            add_slave_psk: String::new(),
            add_slave_step: crate::federation::AddSlaveStep::Idle,
            add_slave_job: Default::default(),
            add_slave_probe: None,
            scan_results: Vec::new(),
            scan_busy: false,
            slave_settings_target: None,
            slave_settings_call_timeout: 45,
            slave_settings_script_timeout: 120,
            slave_settings_error: None,
            show_add_slave_psk: false,
            scan_rx: None,
            slave_self_message: None,
            scan_purpose: crate::wire::ScanPurpose::AddSlave,
            confirm_turn_off_sharing: false,
            focus_master_psk: false,
            needs_restart: false,
            settings_loaded_snapshot,
            known_slave_ids: HashSet::new(),
            slaves_seen_once: false,
            logs_view: LogsViewState::default(),
            dashboard_open: dash_open,
            dash_open_prev: false,
            dash_tab,
            error_ring: Vec::new(),
            last_crash: None,
            crash_count: 0,
            crash_seen: false,
            crash_scan_at: None,
            confirm_stop: false,
            snacks: Vec::new(),
            window_icon,
            recent_projects,
            spawn_busy: false,
            spawn_rx: None,
            palette: crate::palette::PaletteView::default(),
            show_recent_menu: false,
            recent_menu_anchor: None,
        })
    }

    /// Push an action acknowledgment (bottom-right stack, capped at 3).
    pub(crate) fn snack(&mut self, msg: &str, tone: SnackTone) {
        self.snacks.push(Snack {
            msg: msg.to_owned(),
            tone,
            at: Instant::now(),
        });
        if self.snacks.len() > 3 {
            self.snacks.remove(0);
        }
    }

    /// True when the edited draft differs from the loaded config snapshot.
    pub(crate) fn config_dirty(&self) -> bool {
        config_dirty(&self.draft, &self.settings_loaded_snapshot)
            || self.path_buffers() != path_edits_from(&self.draft)
    }

    pub(crate) fn open_settings(&mut self) {
        match cfgfile::load(&self.config_path) {
            Ok(draft) => {
                self.draft = draft;
                self.settings_error = None;
            }
            Err(e) => {
                self.settings_error = Some(format!("load failed: {e}"));
                return;
            }
        }
        self.sync_path_buffers();
        self.settings_loaded_snapshot = self.draft.clone();
        self.confirm_turn_off_sharing = false;
    }

    fn sync_path_buffers(&mut self) {
        let e = path_edits_from(&self.draft);
        self.data_dir_edit = e.data_dir;
        self.bridge_dir_edit = e.bridge_dir;
        self.catalog_path_edit = e.catalog_path;
        self.daemon_bin_edit = e.daemon_bin;
        self.template_path_edit = e.template_path;
        self.official_td_exe_edit = e.official_td_exe;
        self.official_expand_edit = e.official_expand;
        self.official_collapse_edit = e.official_collapse;
        self.official_wine_exe_edit = e.official_wine_exe;
        self.official_wine_prefix_edit = e.official_wine_prefix;
    }

    fn path_buffers(&self) -> PathEdits {
        PathEdits {
            data_dir: self.data_dir_edit.clone(),
            bridge_dir: self.bridge_dir_edit.clone(),
            catalog_path: self.catalog_path_edit.clone(),
            daemon_bin: self.daemon_bin_edit.clone(),
            template_path: self.template_path_edit.clone(),
            official_td_exe: self.official_td_exe_edit.clone(),
            official_expand: self.official_expand_edit.clone(),
            official_collapse: self.official_collapse_edit.clone(),
            official_wine_exe: self.official_wine_exe_edit.clone(),
            official_wine_prefix: self.official_wine_prefix_edit.clone(),
        }
    }

    /// Lazily resolve the log directory once, from either surface.
    pub(crate) fn ensure_logs_dir(&mut self) {
        if self.logs_view.dir.is_some() {
            return;
        }
        self.ensure_base();
        let bearer = local_master_psk(&self.draft);
        if let Ok(body) = http_get_blocking(
            &format!("{}/admin/logs/path", self.admin_base),
            bearer.as_deref(),
        ) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                self.logs_view.dir = v["dir"].as_str().map(str::to_owned);
            }
        }
    }

    /// Logs streaming is wanted while the dashboard sits on its Logs tab —
    /// even when the popup itself is hidden.
    pub(crate) fn logs_surface_active(&self) -> bool {
        self.dashboard_open && self.dash_tab == dashboard::DashTab::Logs
    }

    pub(crate) fn reveal_logs_dir(&self) {
        let Some(dir) = self.logs_view.dir.as_ref() else {
            return;
        };
        let path = PathBuf::from(dir);
        if let Err(e) = reveal_in_file_manager(&path, &path) {
            warn!(error = %e, "reveal logs dir failed");
        }
    }

    /// Fetch the next page of `/admin/logs` when due (either surface wants
    /// logs, not paused) — piggybacks the existing repaint tick, same
    /// throttle style as `poll()`.
    pub(crate) fn fetch_logs_if_due(&mut self) {
        if let Some(result) = self.logs_fetch.poll() {
            self.finish_logs_fetch(result);
        }
        if self.logs_fetch.is_running() {
            return;
        }
        if !self.logs_surface_active() || self.logs_view.paused {
            return;
        }
        let due = self
            .logs_view
            .last_fetch
            .is_none_or(|t| t.elapsed() > Duration::from_millis(250));
        if !due {
            return;
        }
        self.logs_view.last_fetch = Some(Instant::now());
        self.ensure_base();
        let mut url = format!(
            "{}/admin/logs?after={}&limit={LOGS_FETCH_LIMIT}",
            self.admin_base, self.logs_view.next
        );
        if let Some(level) = self.logs_view.min_level {
            url.push_str("&level=");
            url.push_str(level);
        }
        if !self.logs_view.srcs.is_empty() {
            url.push_str("&src=");
            url.push_str(
                &self
                    .logs_view
                    .srcs
                    .iter()
                    .copied()
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        let bearer = local_master_psk(&self.settings_loaded_snapshot);
        if let Err(error) = self.logs_fetch.start("tdmcp-logs", move || {
            http_get_blocking(&url, bearer.as_deref())
        }) {
            self.logs_view.fetch_error = Some(error);
        }
    }

    fn finish_logs_fetch(&mut self, result: Result<String, String>) {
        match result {
            Ok(body) => match serde_json::from_str::<LogsResponse>(&body) {
                Ok(page) => {
                    self.logs_view.fetch_error = None;
                    self.logs_view.next = page.next;
                    for r in page.records {
                        self.logs_view.buf.push_back(r);
                        if self.logs_view.buf.len() > LOGS_RENDER_CAP {
                            self.logs_view.buf.pop_front();
                        }
                    }
                }
                Err(e) => self.logs_view.fetch_error = Some(e.to_string()),
            },
            Err(e) => self.logs_view.fetch_error = Some(e),
        }
    }

    /// Changing filters resets the client buffer + cursor to 0 so the next
    /// fetch re-derives the tail from whatever the ring still holds, under
    /// the new filter — matches the plan's "changing filters resets cursor
    /// and refetches the tail" contract.
    pub(crate) fn reset_logs_filter_state(&mut self) {
        // Drop the receiver so an old filter's in-flight page cannot leak in.
        self.logs_fetch = Default::default();
        self.logs_view.buf.clear();
        self.logs_view.next = 0;
        self.logs_view.expanded = None;
        self.logs_view.last_fetch = None;
    }

    /// Toggle LAN reachability. Auth is a separate, optional choice (see the
    /// Auth PSK row) — sharing does not require or imply a PSK. Turning sharing
    /// off also drops federation role back to standalone — leaving
    /// `role=master`/`slave` on a loopback-only bind would silently produce a dead
    /// federation link with no error surfaced anywhere.
    pub(crate) fn set_sharing(&mut self, on: bool) {
        self.draft.server.bind_address = if on { "0.0.0.0" } else { "127.0.0.1" }.to_owned();
        if !on {
            self.draft.federation.role = "standalone".to_owned();
        }
        self.confirm_turn_off_sharing = false;
    }

    fn apply_path_edits(&mut self) {
        self.draft.advanced.data_dir = nonempty_path(&self.data_dir_edit);
        self.draft.advanced.bridge_dir = nonempty_path(&self.bridge_dir_edit);
        self.draft.advanced.catalog_path = nonempty_path(&self.catalog_path_edit);
        self.draft.advanced.daemon_bin = nonempty_path(&self.daemon_bin_edit);
        self.draft.project.template_path = nonempty_path(&self.template_path_edit);
        self.draft.official_tools.td_exe = nonempty_path(&self.official_td_exe_edit);
        self.draft.official_tools.expand_path = nonempty_path(&self.official_expand_edit);
        self.draft.official_tools.collapse_path = nonempty_path(&self.official_collapse_edit);
        self.draft.official_tools.wine_exe = nonempty_opt(&self.official_wine_exe_edit);
        self.draft.official_tools.wine_prefix = nonempty_path(&self.official_wine_prefix_edit);
    }

    pub(crate) fn save_settings(&mut self) {
        if self.settings_save.is_running() {
            return;
        }
        self.apply_path_edits();
        if let Err(e) = cfgfile::validate(&self.draft) {
            self.settings_error = Some(e.to_string());
            return;
        }
        if self.status.is_some() {
            let base = self.admin_base.clone();
            let loaded = self.settings_loaded_snapshot.clone();
            let draft = self.draft.clone();
            self.settings_error = None;
            if let Err(error) = self.settings_save.start("tdmcp-save", move || {
                let patch = cfgfile::diff(&loaded, &draft).map_err(|e| e.to_string())?;
                let reply = http_post_blocking(
                    &format!("{base}/admin/config"),
                    local_master_psk(&loaded).as_deref(),
                    Some(&patch),
                )?;
                let restart = reply
                    .get("restartRequired")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|a| !a.is_empty());
                let config = reply
                    .get("config")
                    .ok_or_else(|| "daemon did not return saved settings".to_owned())?;
                let saved = serde_json::from_value(config.clone()).map_err(|e| e.to_string())?;
                Ok((saved, restart))
            }) {
                self.settings_error = Some(error);
            }
        } else {
            let restart =
                restart_required_fields_changed(&self.settings_loaded_snapshot, &self.draft);
            let result = cfgfile::save(&self.config_path, &self.draft)
                .map(|()| (self.draft.clone(), restart))
                .map_err(|e| e.to_string());
            self.finish_settings_save(result);
        }
    }

    fn finish_settings_save(&mut self, result: Result<(ConfigFile, bool), String>) {
        match result {
            Ok((saved, restart_needed)) => {
                self.draft = saved;
                self.settings_error = None;
                self.role_change_note = None;
                self.needs_restart = restart_needed;
                self.settings_loaded_snapshot = self.draft.clone();
                self.sync_path_buffers();
                self.last_poll = None;
                self.snack(
                    "Settings saved",
                    if restart_needed {
                        SnackTone::Warn
                    } else {
                        SnackTone::Ok
                    },
                );
            }
            Err(e) => {
                self.settings_error = Some(format!("save failed: {e}"));
                self.snack("Save failed", SnackTone::Error);
            }
        }
    }

    pub(crate) fn discard_settings(&mut self) {
        self.draft = self.settings_loaded_snapshot.clone();
        self.sync_path_buffers();
        self.settings_error = None;
        self.role_change_note = None;
    }

    pub(crate) fn reset_settings(&mut self) {
        let identity = self.draft.federation.daemon_id.clone();
        let binary = self.draft.advanced.daemon_bin.clone();
        self.draft = ConfigFile::default();
        self.draft.federation.daemon_id = identity;
        self.draft.advanced.daemon_bin = binary;
        self.sync_path_buffers();
        self.settings_error = None;
    }

    pub(crate) fn quitting(&self) -> bool {
        self.quit.load(Ordering::SeqCst)
    }

    pub(crate) fn ensure_base(&mut self) {
        if self.admin_base.is_empty() {
            self.admin_base = format!("http://127.0.0.1:{}", tdmcp_config::DEFAULT_PORT);
        }
    }

    pub(crate) fn hide_window(&mut self, ctx: &egui::Context) {
        self.visible = false;
        self.clear_always_on_top_at = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::Normal,
        ));
    }

    pub(crate) fn show_window(&mut self, ctx: &egui::Context, tray_rect: Option<TrayRect>) {
        if let Some(r) = tray_rect {
            self.last_tray_rect = Some(r);
        }
        self.visible = true;
        self.tray_popup_close_on_up = false;
        self.position_near_tray(ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        // Transient always-on-top to win z-order (Docker-style), then drop.
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::AlwaysOnTop,
        ));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        let now = Instant::now();
        self.clear_always_on_top_at = Some(now + Duration::from_millis(150));
        self.ignore_focus_loss_until = Some(now + Duration::from_millis(400));
        // Outside-click close re-arms on the first all-buttons-up poll after
        // show, so the press that opened the popup can never close it.
        self.outside_click_armed = false;
        self.press_inside_active = false;
        // Open the X11 poll connection on first popup show (stays `None` off
        // X11 or when the server is unreachable — focus-loss hide keeps working).
        if self.x11_pointer.is_none() {
            self.x11_pointer = crate::platform::glance_pointer::PointerPoller::open();
        }
    }

    pub(crate) fn handle_close_request(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.quitting() {
            // Real shutdown — allow the viewport to close.
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.hide_window(ctx);
    }

    pub(crate) fn handle_focus_loss(&mut self, ctx: &egui::Context) {
        if !self.visible {
            return;
        }
        // On X11 the outside-click poll owns popup closing: under
        // focus-follows-mouse WMs (Hyprland default) a mere cursor move also
        // steals focus, which read as "click outside". Keep this fallback for
        // the Wayland backend and other platforms.
        if crate::using_x11_backend() && self.x11_pointer.is_some() {
            return;
        }
        // Any focus loss hides the popup; editing happens in the dashboard.
        if self
            .ignore_focus_loss_until
            .is_some_and(|t| Instant::now() < t)
        {
            return;
        }
        let focused = ctx.input(|i| i.viewport().focused);
        if focused == Some(false) {
            self.hide_window(ctx);
        }
    }

    /// The glance was just closed by an outside click — the tray Up half of
    /// the same physical click must not re-arm the popup (anti-reopen).
    pub(crate) fn recent_outside_click_close(&self) -> bool {
        self.outside_click_close_at
            .is_some_and(|t| t.elapsed() < OUTSIDE_CLOSE_TOGGLE_GUARD)
    }

    /// X11: close the popup when a physical button is held outside it.
    ///
    /// A click outside our window is never delivered to us, and under
    /// focus-follows-mouse the focus loss it causes is indistinguishable from
    /// a cursor move — so poll the X server instead (see
    /// `platform::glance_pointer`). Cursor-over-window is tracked from egui's
    /// own events; detection arms on the first all-buttons-up poll after show.
    fn poll_outside_click_close(&mut self, ctx: &egui::Context) {
        if !crate::using_x11_backend() || !self.visible {
            return;
        }
        let mut inside = self.pointer_inside;
        let mut pressed_inside = false;
        ctx.input(|i| {
            for e in &i.events {
                match e {
                    egui::Event::PointerMoved(_) => inside = true,
                    egui::Event::PointerGone => inside = false,
                    egui::Event::PointerButton { pressed: true, .. } => {
                        pressed_inside = true;
                        inside = true;
                    }
                    _ => {}
                }
            }
        });
        self.pointer_inside = inside;
        if pressed_inside {
            self.press_inside_active = true;
        }
        let Some(poller) = self.x11_pointer.as_ref() else {
            return;
        };
        let Some(mask) = poller.held_buttons() else {
            return;
        };
        if mask == 0 {
            // All buttons up: arm detection and end any drag that started
            // inside the popup.
            self.outside_click_armed = true;
            self.press_inside_active = false;
            return;
        }
        if self.outside_click_armed && !self.pointer_inside && !self.press_inside_active {
            self.hide_window(ctx);
            self.outside_click_close_at = Some(Instant::now());
        }
    }

    /// Tray double-click / menu: open the dashboard or raise/focus it when
    /// already open.
    pub(crate) fn open_or_focus_dashboard(&mut self, ctx: &egui::Context) {
        let id = dashboard::viewport_id();
        if self.dashboard_open {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        } else {
            self.dashboard_open = true;
            // The glance used to close via focus loss when the dashboard took
            // focus; keep that behaviour now that focus loss no longer hides
            // it on X11.
            if self.visible {
                self.hide_window(ctx);
            }
        }
    }

    /// Refresh `last_crash`/`crash_count` from `{data_dir}/crash`, at most
    /// every [`CRASH_SCAN_INTERVAL`] — the daemon writes reports there on
    /// panic (see `tdmcp_daemon::crashreport`).
    fn scan_crash_reports(&mut self) {
        if self
            .crash_scan_at
            .is_some_and(|t| t.elapsed() < CRASH_SCAN_INTERVAL)
        {
            return;
        }
        self.crash_scan_at = Some(Instant::now());
        let dir = self.data_dir.join("crash");
        let mut reports: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "log") {
                    reports.push(p);
                }
            }
        }
        self.crash_count = reports.len();
        // Filename-encoded timestamp: lexicographic max is the newest.
        reports.sort();
        self.last_crash = reports.pop();
    }

    pub(crate) fn poll(&mut self) {
        self.ensure_base();
        self.scan_crash_reports();
        let snapshot = if let Some(rx) = &self.poll_rx {
            match rx.try_recv() {
                Ok(snapshot) => {
                    self.poll_rx = None;
                    snapshot
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.poll_rx = None;
                    self.error = Some("Status worker stopped".to_owned());
                    self.last_poll = Some(Instant::now());
                    return;
                }
            }
        } else {
            let (tx, rx) = std::sync::mpsc::channel();
            let base = self.admin_base.clone();
            let bearer = local_master_psk(&self.settings_loaded_snapshot);
            match std::thread::Builder::new()
                .name("tdmcp-status".into())
                .spawn(move || {
                    let _ = tx.send(crate::http::poll_snapshot(&base, bearer.as_deref()));
                }) {
                Ok(_) => self.poll_rx = Some(rx),
                Err(error) => self.error = Some(format!("Could not refresh status: {error}")),
            }
            self.last_poll = Some(Instant::now());
            return;
        };
        let status_ok = match snapshot.status {
            Ok(body) => match serde_json::from_str::<StatusView>(&body) {
                Ok(status) => {
                    self.needs_restart = !status.restart_required.is_empty();
                    self.status = Some(status);
                    self.error = None;
                    true
                }
                Err(error) => {
                    self.status = None;
                    self.error = Some(format!("Invalid status response: {error}"));
                    false
                }
            },
            Err(e) => {
                self.status = None;
                self.error = Some(e);
                false
            }
        };
        match snapshot.fleet {
            Ok(body) => {
                self.fleet_json = body;
                self.apply_fleet_status();
            }
            Err(e) => {
                self.error = Some(e.clone());
                push_error_ring(&mut self.error_ring, e);
            }
        }
        match snapshot.sessions {
            Ok(body) => self.sessions_json = body,
            Err(e) => {
                if self.error.is_none() {
                    self.error = Some(e.clone());
                }
                push_error_ring(&mut self.error_ring, e);
            }
        }

        let is_master = self
            .status
            .as_ref()
            .is_some_and(|s| s.role.eq_ignore_ascii_case("master"));
        if is_master {
            match snapshot.slaves {
                Ok(body) => {
                    self.slaves_json = body;
                    let slaves = parse_slaves(&self.slaves_json);
                    if self.slaves_seen_once {
                        for s in &slaves {
                            if !self.known_slave_ids.contains(&s.daemon_id) {
                                notify("Slave joined", &s.hostname);
                            }
                        }
                    }
                    self.known_slave_ids = slaves.iter().map(|s| s.daemon_id.clone()).collect();
                    self.slaves_seen_once = true;
                }
                Err(error) => self.error = Some(error),
            }
        } else {
            self.slaves_json.clear();
            self.known_slave_ids.clear();
            self.slaves_seen_once = false;
        }

        if status_ok {
            self.fail_polls = 0;
            if !self.startup_notified {
                self.startup_notified = true;
                notify(
                    "td-mcp-rs",
                    &format!("listening on {}", self.admin_base.trim_end_matches('/')),
                );
            }
        } else if !self.startup_notified {
            self.fail_polls = self.fail_polls.saturating_add(1);
            if self.fail_polls >= 2 && !self.startup_fail_notified {
                self.startup_fail_notified = true;
                notify(
                    "td-mcp-rs",
                    "daemon not reachable — check bind / already running",
                );
            }
        }

        self.last_poll = Some(Instant::now());
    }

    pub(crate) fn apply_fleet_status(&mut self) {
        let Ok(fleet) = serde_json::from_str::<FleetView>(&self.fleet_json) else {
            return;
        };
        let mut snap = FleetSnapshot::default();
        for p in &fleet.processes {
            let bridge = p.bridge.as_str().unwrap_or("");
            match bridge {
                "connected" => {
                    snap.connected += 1;
                    snap.connected_pids.push(p.pid);
                }
                "disconnected" => snap.disconnected += 1,
                _ => {}
            }
            if p.resurrected {
                snap.resurrected += 1;
                snap.resurrected_pids.push(p.pid);
            }
            snap.cancelled += p.cancelled_tasks.len();
        }
        snap.cancelled_total = snap.cancelled;

        if self.startup_notified {
            for pid in &snap.resurrected_pids {
                if !self.prev_snapshot.resurrected_pids.contains(pid) {
                    notify(
                        "Bridge resurrected",
                        &format!("pid {pid} reconnected — check cancelled tasks"),
                    );
                    push_error_ring(
                        &mut self.error_ring,
                        format!("bridge resurrected — pid {pid} reconnected"),
                    );
                }
            }
            for pid in &self.prev_snapshot.connected_pids {
                if !snap.connected_pids.contains(pid) {
                    notify(
                        "Bridge disconnected",
                        &format!("pid {pid} lost IPC — tasks cancelled"),
                    );
                    push_error_ring(
                        &mut self.error_ring,
                        format!("bridge disconnected — pid {pid} lost IPC"),
                    );
                }
            }
            if snap.cancelled_total > self.prev_snapshot.cancelled_total {
                let delta = snap.cancelled_total - self.prev_snapshot.cancelled_total;
                notify(
                    "Tasks cancelled",
                    &format!("{delta} task(s) stacked on bridge loss"),
                );
                push_error_ring(
                    &mut self.error_ring,
                    format!("{delta} task(s) cancelled on bridge loss"),
                );
            }
        }

        let needs_attention = snap.disconnected > 0 || snap.resurrected > 0 || snap.cancelled > 0;
        let mcp_n = self
            .status
            .as_ref()
            .map(|s| s.mcp_session_count)
            .unwrap_or(0);
        let tooltip = if snap.connected + snap.disconnected == 0 && mcp_n == 0 {
            "td-mcp-rs — no connections".to_owned()
        } else if needs_attention {
            format!(
                "td-mcp-rs — MCP {mcp_n}, TD {} connected, {} attention",
                snap.connected,
                snap.disconnected + snap.resurrected + snap.cancelled
            )
        } else {
            format!("td-mcp-rs — MCP {mcp_n}, TD {} connected", snap.connected)
        };

        if let Some(tray) = &self.tray {
            tray.set_tooltip(&tooltip);
            if needs_attention != self.attention {
                let icon = if needs_attention {
                    &self.icon_attention
                } else {
                    &self.icon_normal
                };
                tray.set_icon(icon);
                self.attention = needs_attention;
            }
        }

        self.prev_snapshot = snap;
    }

    pub(crate) fn shutdown_daemon(&mut self) {
        self.ensure_base();
        let _ = http_post_blocking(&format!("{}/admin/shutdown", self.admin_base), None, None);
        self.confirm_stop = false;
        self.snack("Shutdown issued", SnackTone::Warn);
    }

    pub(crate) fn restart_daemon(&mut self) {
        self.ensure_base();
        match http_post_blocking(&format!("{}/admin/restart", self.admin_base), None, None) {
            Ok(_) => {
                self.confirm_stop = false;
                self.needs_restart = false;
                self.snack("Restarting…", SnackTone::Info);
            }
            Err(error) => {
                self.settings_error = Some(error);
                self.snack("Restart failed", SnackTone::Error);
            }
        }
    }

    pub(crate) fn reveal_tox(&self) {
        if let Err(e) = reveal_in_file_manager(&self.data_dir.join("bootstrap.tox"), &self.data_dir)
        {
            warn!(error = %e, "reveal bootstrap.tox failed");
        }
    }

    pub(crate) fn effective_template_path(&self) -> PathBuf {
        if let Some(p) = &self.draft.project.template_path {
            return p.clone();
        }
        if let Some(p) = nonempty_path(&self.template_path_edit) {
            return p;
        }
        // Respect [advanced].data_dir override.
        let base = self
            .draft
            .advanced
            .data_dir
            .clone()
            .unwrap_or_else(|| self.data_dir.clone());
        base.join("template.toe")
    }

    pub(crate) fn reveal_template(&self) {
        let tmpl = self.effective_template_path();
        let fallback = tmpl
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.data_dir.clone());
        if let Err(e) = reveal_in_file_manager(&tmpl, &fallback) {
            warn!(error = %e, "reveal template.toe failed");
        }
    }

    pub(crate) fn locate_template(&mut self) {
        let start = self
            .effective_template_path()
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.data_dir.clone());
        if let Some(path) = rfd::FileDialog::new()
            .set_directory(&start)
            .add_filter("TouchDesigner Project", &["toe", "tox"])
            .pick_file()
        {
            self.template_path_edit = path.display().to_string();
            self.draft.project.template_path = nonempty_path(&self.template_path_edit);
        }
    }

    pub(crate) fn open_template(&self) {
        let tmpl = self.effective_template_path();
        if !tmpl.is_file() {
            warn!(path = %tmpl.display(), "open template: file not found");
            return;
        }
        // Use OS default association: `open` / `explorer` / `xdg-open`.
        let res = {
            #[cfg(target_os = "windows")]
            {
                std::process::Command::new("explorer")
                    .arg(tmpl.as_os_str())
                    .spawn()
                    .map(|_| ())
            }
            #[cfg(target_os = "macos")]
            {
                std::process::Command::new("open")
                    .arg(tmpl.as_os_str())
                    .spawn()
                    .map(|_| ())
            }
            #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
            {
                std::process::Command::new("xdg-open")
                    .arg(tmpl.as_os_str())
                    .spawn()
                    .map(|_| ())
            }
        };
        if let Err(e) = res {
            warn!(error = %e, path = %tmpl.display(), "open template failed");
        }
    }

    pub(crate) fn field_help(key: &str) -> &'static str {
        FIELD_DESCS
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.help)
            .unwrap_or("")
    }

    // ---- Recent projects + spawn helpers (F1/F2) ----

    pub(crate) fn save_recent(&self) {
        crate::recent::save(&self.data_dir, &self.recent_projects);
    }

    pub(crate) fn push_recent(&mut self, path: PathBuf) {
        crate::recent::push(&mut self.recent_projects, path);
        self.save_recent();
    }

    /// Fire an async `spawn_td` via the daemon JSON fallback (`POST /mcp/tools/call`).
    /// De-duplicates into recents on success.
    pub(crate) fn spawn_project(&mut self, path: PathBuf, create_if_missing: bool) {
        if self.spawn_busy {
            self.snack("Spawn already in flight", SnackTone::Warn);
            return;
        }
        self.spawn_busy = true;
        self.snack(
            &format!(
                "{} {}",
                if create_if_missing {
                    "Creating"
                } else {
                    "Opening"
                },
                path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string())
            ),
            SnackTone::Info,
        );
        let admin_base = self.admin_base.clone();
        let draft = self.draft.clone();
        let path_clone = path.clone();
        let (tx, rx) = std::sync::mpsc::channel::<Result<serde_json::Value, String>>();
        self.spawn_rx = Some(rx);
        std::thread::spawn(move || {
            let res = spawn_via_mcp(&admin_base, &draft, &path_clone, create_if_missing);
            let _ = tx.send(res);
        });
        // Optimistically push to recents (visible immediately); dedup handles reorder.
        self.push_recent(path);
    }

    pub(crate) fn poll_spawn(&mut self) {
        let Some(rx) = self.spawn_rx.as_ref() else {
            return;
        };
        if let Ok(res) = rx.try_recv() {
            self.spawn_busy = false;
            self.spawn_rx = None;
            match res {
                Ok(v) => {
                    // HTTP layer wraps tool result as {ok: bool, data?: {...}, summary?: ...}.
                    // For spawn, success is {ok:true, data:{ok:true, pid...}} and a
                    // wait_timeout is {ok:true, data:{ok:false, outcome:"wait_timeout",...}}.
                    // Unwrap the inner `data` when present.
                    let outer_ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
                    let inner = v.get("data").unwrap_or(&v);
                    let inner_ok = inner.get("ok").and_then(|x| x.as_bool());
                    let effective_ok = match inner_ok {
                        Some(b) if v.get("data").is_some() => b && outer_ok,
                        _ => outer_ok,
                    };
                    if effective_ok {
                        self.snack("TouchDesigner spawned", SnackTone::Ok);
                    } else {
                        let outcome = inner.get("outcome").and_then(|x| x.as_str()).unwrap_or("");
                        let msg = inner
                            .get("summary")
                            .and_then(|x| x.as_str())
                            .or_else(|| v.get("summary").and_then(|x| x.as_str()))
                            .or_else(|| {
                                if outcome == "wait_timeout" {
                                    Some("handshake timed out — still starting? check fleet")
                                } else if !outcome.is_empty() {
                                    Some(outcome)
                                } else {
                                    None
                                }
                            })
                            .unwrap_or("spawn returned ok:false");
                        // Distinguish a real failure from a timeout where the
                        // process is still alive — the fleet will show it
                        // shortly and the next poll recovers, so keep tone Info
                        // and avoid polluting the error ring with a transient
                        // transport message like "error sending request for url…".
                        let tone = if outcome == "wait_timeout" {
                            SnackTone::Warn
                        } else {
                            SnackTone::Error
                        };
                        self.snack(&format!("Spawn: {msg}"), tone);
                        if tone == SnackTone::Error {
                            push_error_ring(&mut self.error_ring, format!("spawn failed: {msg}"));
                        }
                    }
                }
                Err(e) => {
                    // Transport error (daemon unreachable, not a spawn payload).
                    // The project was already created on disk from the template
                    // before the HTTP round-trip, so a short "error sending
                    // request" no longer means the .toe was lost — the next fleet
                    // poll will show the new pid. Keep the snack but don't spam
                    // the ring with reqwest internals.
                    if e.contains("error sending request") || e.contains("timed out") {
                        self.snack(
                            "Spawn issued — waiting for TouchDesigner (fleet will update shortly)",
                            SnackTone::Info,
                        );
                    } else {
                        self.snack(&format!("Spawn failed: {e}"), SnackTone::Error);
                        push_error_ring(&mut self.error_ring, format!("spawn failed: {e}"));
                    }
                }
            }
        }
    }

    pub(crate) fn open_project_dialog(&mut self) {
        let start = self
            .recent_projects
            .first()
            .and_then(|p| p.parent().map(PathBuf::from))
            .or_else(|| self.effective_template_path().parent().map(PathBuf::from))
            .unwrap_or_else(|| self.data_dir.clone());
        if let Some(path) = rfd::FileDialog::new()
            .set_directory(&start)
            .add_filter("TouchDesigner", &["toe", "tox"])
            .pick_file()
        {
            self.spawn_project(path, false);
        }
    }

    pub(crate) fn create_project_dialog(&mut self) {
        let start = self
            .recent_projects
            .first()
            .and_then(|p| p.parent().map(PathBuf::from))
            .or_else(|| self.effective_template_path().parent().map(PathBuf::from))
            .unwrap_or_else(|| self.data_dir.clone());
        if let Some(path) = rfd::FileDialog::new()
            .set_directory(&start)
            .add_filter("TouchDesigner", &["toe", "tox"])
            .set_file_name("Untitled.toe")
            .save_file()
        {
            self.spawn_project(path, true);
        }
    }
}

fn spawn_via_mcp(
    admin_base: &str,
    draft: &ConfigFile,
    path: &std::path::Path,
    create_if_missing: bool,
) -> Result<serde_json::Value, String> {
    let url = format!("{}/mcp/tools/call", admin_base.trim_end_matches('/'));
    // Give TouchDesigner time to cold-start, show any startup dialogs,
    // and complete the bridge handshake. Default wait is 60s (see
    // `crates/tdmcp-mcp/src/lifecycle.rs::default_wait`); the GUI's 3s
    // generic POST timeout is far too short — seen as
    // `error sending request for url (.../mcp/tools/call)` even though
    // the spawn succeeded and fleet later shows the pid.
    let wait_ms: u64 = 65_000;
    let body = serde_json::json!({
        "name": "spawn_td",
        "arguments": {
            "projectPath": path.display().to_string(),
            "createIfMissing": create_if_missing,
            "waitTimeoutMs": wait_ms
        }
    });
    let bearer = if draft.auth.mode == "psk" && !draft.auth.psk.is_empty() {
        Some(draft.auth.psk.clone())
    } else {
        None
    };
    crate::http::http_post_blocking_with_timeout(
        &url,
        bearer.as_deref(),
        Some(&body),
        Duration::from_millis(wait_ms + 10_000),
    )
}

impl eframe::App for DashboardApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.quitting() {
            // Drop the tray before closing so the icon does not linger.
            self.tray = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            self.handle_close_request(ctx);
            return;
        }
        self.ensure_tray();
        if self.pending_initial_hide {
            self.pending_initial_hide = false;
            self.hide_window(ctx);
        }
        if let Some(at) = self.clear_always_on_top_at {
            if Instant::now() >= at {
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                    egui::WindowLevel::Normal,
                ));
                self.clear_always_on_top_at = None;
            }
        }
        self.handle_close_request(ctx);
        self.handle_tray_events(ctx);
        self.flush_pending_tray_popup(ctx);
        self.handle_focus_loss(ctx);
        self.poll_outside_click_close(ctx);
        let due = self
            .last_poll
            .is_none_or(|t| t.elapsed() > Duration::from_secs(2));
        if due || self.poll_rx.is_some() {
            self.poll();
        }
        self.fetch_logs_if_due();
        self.poll_add_pipeline();
        if let Some(result) = self.settings_save.poll() {
            self.finish_settings_save(result);
        }
        let scan_hits = self.scan_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(hits) = scan_hits {
            let n = hits.len();
            self.scan_results = hits;
            self.scan_busy = false;
            self.scan_rx = None;
            self.snack(&format!("Scan finished · {n} found"), SnackTone::Info);
        }
        self.poll_spawn();
        crate::palette::poll(self);
        // Keep the dashboard viewport alive from the very first tick and only
        // toggle its visibility: when the root window is hidden, eframe drives
        // its repaints outside the winit event-loop guard, and an immediate
        // viewport created on such a tick can never get its native window
        // (egui then panics "the user callback was never called", crashing
        // the daemon). The window is born during eframe's guarded first paint;
        // after that, showing/hiding it from any tick is safe.
        if self.dash_open_prev != self.dashboard_open {
            let id = dashboard::viewport_id();
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Visible(self.dashboard_open));
            if self.dashboard_open {
                ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
            }
        }
        self.dash_open_prev = self.dashboard_open;
        // Builder `visible` is synced by the backend each frame — mirror the
        // open flag so the pre-created window stays hidden until opened.
        let vb = dashboard::builder(&self.window_icon).with_visible(self.dashboard_open);
        ctx.show_viewport_immediate(dashboard::viewport_id(), vb, |ui, _class| {
            if self.dashboard_open {
                dashboard::render(self, ui);
            }
        });
        // 100 ms while the glance is open = outside-click poll cadence.
        let cadence = if self.visible {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(250)
        };
        ctx.request_repaint_after(cadence);
    }

    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        eframe::egui::CentralPanel::default()
            .frame(
                eframe::egui::Frame::NONE
                    .fill(crate::theme::BG_WINDOW)
                    .stroke(eframe::egui::Stroke::new(1.0, crate::theme::BORDER))
                    .inner_margin(0.0),
            )
            .show(ui, |ui| {
                self.draw_header(ui);
                // Footer is pinned chrome: reserve its height out of the
                // scroll budget and render it after, never inside.
                eframe::egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    // Actual remaining space, not a WINDOW_MAX_HEIGHT-derived
                    // cap: with auto_shrink(false) the area fills max_height,
                    // which pushed the footer off-screen in short windows.
                    .max_height((ui.available_height() - crate::popup::FOOTER_BLOCK_H).max(0.0))
                    .show(ui, |ui| {
                        ui.add_space(crate::theme::sp::SM);
                        self.draw_summary(ui);
                    });
                self.draw_action_footer(ui);
            });
    }
}

/// Settings-tab text buffers derived from the draft config — named fields,
/// never positional, so adding a row can't silently transpose two of them.
#[derive(PartialEq, Eq)]
struct PathEdits {
    data_dir: String,
    bridge_dir: String,
    catalog_path: String,
    daemon_bin: String,
    template_path: String,
    official_td_exe: String,
    official_expand: String,
    official_collapse: String,
    official_wine_exe: String,
    official_wine_prefix: String,
}

fn path_edit(p: Option<&Path>) -> String {
    p.map(|p| p.display().to_string()).unwrap_or_default()
}

fn path_edits_from(cfg: &ConfigFile) -> PathEdits {
    let t = &cfg.official_tools;
    PathEdits {
        data_dir: path_edit(cfg.advanced.data_dir.as_deref()),
        bridge_dir: path_edit(cfg.advanced.bridge_dir.as_deref()),
        catalog_path: path_edit(cfg.advanced.catalog_path.as_deref()),
        daemon_bin: path_edit(cfg.advanced.daemon_bin.as_deref()),
        template_path: path_edit(cfg.project.template_path.as_deref()),
        official_td_exe: path_edit(t.td_exe.as_deref()),
        official_expand: path_edit(t.expand_path.as_deref()),
        official_collapse: path_edit(t.collapse_path.as_deref()),
        official_wine_exe: t.wine_exe.clone().unwrap_or_default(),
        official_wine_prefix: path_edit(t.wine_prefix.as_deref()),
    }
}

fn nonempty_path(s: &str) -> Option<PathBuf> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(PathBuf::from(t))
    }
}

/// Push a message onto the newest-first error ring, deduping a repeated head.
pub(crate) fn push_error_ring(ring: &mut Vec<String>, msg: String) {
    if ring.first().is_some_and(|m| *m == msg) {
        return;
    }
    ring.insert(0, msg);
    ring.truncate(ERROR_RING_CAP);
}

/// Dirty check for the Settings editor (`ConfigFile` derives `PartialEq`).
#[must_use]
pub(crate) fn config_dirty(a: &ConfigFile, b: &ConfigFile) -> bool {
    a != b
}

/// True when a field that only takes effect after a daemon restart changed.
#[must_use]
pub(crate) fn restart_required_fields_changed(a: &ConfigFile, b: &ConfigFile) -> bool {
    !cfgfile::restart_required_fields(a, b).is_empty()
}

/// Random PSK for `auth.psk` (32 hex chars) when the user switches to `psk`.
#[must_use]
pub(crate) fn generate_psk() -> String {
    Uuid::new_v4().simple().to_string()
}

/// Local daemon's own psk to call its psk-gated admin routes, if `mode = psk`.
#[must_use]
pub(crate) fn local_master_psk(cfg: &ConfigFile) -> Option<String> {
    if cfg.auth.mode == "psk" && !cfg.auth.psk.is_empty() {
        Some(cfg.auth.psk.clone())
    } else {
        None
    }
}

#[must_use]
pub(crate) fn nonempty_opt(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_owned())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn error_ring_dedupes_head_and_caps() {
        let mut ring: Vec<String> = Vec::new();
        push_error_ring(&mut ring, "a".to_owned());
        push_error_ring(&mut ring, "a".to_owned());
        assert_eq!(ring.len(), 1);
        push_error_ring(&mut ring, "b".to_owned());
        assert_eq!(ring.first().map(String::as_str), Some("b"));
        for i in 0..(ERROR_RING_CAP + 10) {
            push_error_ring(&mut ring, format!("m{i}"));
        }
        assert_eq!(ring.len(), ERROR_RING_CAP);
    }

    #[test]
    fn config_dirty_tracks_any_section_change() {
        let a = ConfigFile::default();
        let mut b = ConfigFile::default();
        assert!(!config_dirty(&a, &b));
        b.server.port = a.server.port + 1;
        assert!(config_dirty(&a, &b));
        let mut c = ConfigFile::default();
        c.federation.role = "master".to_owned();
        assert!(config_dirty(&a, &c));
    }

    #[test]
    fn discard_restores_path_edits_and_reset_waits_for_save() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let mut cfg = ConfigFile::default();
        cfg.federation.daemon_id = "stable-id".into();
        cfg.bridge.call_timeout_secs = 75;
        cfgfile::save(&path, &cfg).unwrap();
        let before = std::fs::read(&path).unwrap();
        let mut app = headless_app(&path);
        app.open_settings();
        app.template_path_edit = "/tmp/edited.toe".into();
        assert!(app.config_dirty());
        app.discard_settings();
        assert!(!app.config_dirty());
        assert!(app.template_path_edit.is_empty());
        app.reset_settings();
        assert!(app.config_dirty());
        assert_eq!(app.draft.federation.daemon_id, "stable-id");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        app.discard_settings();
        assert_eq!(app.draft, cfg);
    }

    /// A real `DashboardApp` against a scratch config/data dir — the same
    /// construction the `preview` fixture harness uses, minus the window.
    fn headless_app(config_path: &std::path::Path) -> DashboardApp {
        let blank = || RgbaIcon {
            rgba: vec![0, 0, 0, 255],
            width: 1,
            height: 1,
        };
        let mut app = DashboardApp::new(
            "http://127.0.0.1:9860".to_owned(),
            config_path.parent().unwrap().to_path_buf(),
            blank(),
            blank(),
            Arc::new(AtomicBool::new(false)),
            config_path.to_path_buf(),
            egui::IconData {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            },
        )
        .unwrap();
        app.pending_tray = false;
        app.pending_initial_hide = false;
        app.visible = true;
        app.dashboard_open = true;
        app
    }

    #[test]
    fn official_tools_flow_through_settings_open_edit_save() {
        // Config shaped like the AUR touchdesigner-linux machine: td exe +
        // pinned wine build, prefix derived by the resolver at runtime.
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        let mut cfg = cfgfile::load(&config_path).unwrap();
        cfg.official_tools.td_exe =
            Some(PathBuf::from("/opt/touchdesigner/td/bin/TouchDesigner.exe"));
        cfg.official_tools.wine_exe = Some("/opt/touchdesigner/wine/bin/wine64".to_owned());
        cfg.official_tools.wine_prefix = Some(PathBuf::from(
            "/home/u/.local/share/touchdesigner-linux/prefix",
        ));
        cfgfile::save(&config_path, &cfg).unwrap();

        // Opening settings must surface every configured value in the
        // OFFICIAL TOOLS edit buffers (unset rows stay empty).
        let mut app = headless_app(&config_path);
        app.open_settings();
        assert_eq!(
            app.official_td_exe_edit,
            "/opt/touchdesigner/td/bin/TouchDesigner.exe"
        );
        assert_eq!(
            app.official_wine_exe_edit,
            "/opt/touchdesigner/wine/bin/wine64"
        );
        assert_eq!(
            app.official_wine_prefix_edit,
            "/home/u/.local/share/touchdesigner-linux/prefix"
        );
        assert!(app.official_expand_edit.is_empty());
        assert!(app.official_collapse_edit.is_empty());

        // Simulated user edit: clear wine_exe, set a new prefix, save.
        app.official_wine_exe_edit = String::new();
        app.official_wine_prefix_edit = "/home/u/.wine".to_owned();
        app.apply_path_edits();
        assert_eq!(app.draft.official_tools.wine_exe, None);
        assert_eq!(
            app.draft.official_tools.wine_prefix.as_deref(),
            Some(std::path::Path::new("/home/u/.wine"))
        );
        app.save_settings();

        // The saved file round-trips — and clearing a field drops the key.
        let reloaded = cfgfile::load(&config_path).unwrap();
        assert_eq!(
            reloaded.official_tools.td_exe.as_deref(),
            Some(std::path::Path::new(
                "/opt/touchdesigner/td/bin/TouchDesigner.exe"
            ))
        );
        assert_eq!(reloaded.official_tools.wine_exe, None);
        assert_eq!(
            reloaded.official_tools.wine_prefix.as_deref(),
            Some(std::path::Path::new("/home/u/.wine"))
        );
    }

    #[test]
    fn rejected_online_save_keeps_draft_and_does_not_block_the_ui() {
        use std::io::{Read, Write};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        cfgfile::ensure_default(&path, false).unwrap();
        let before = std::fs::read(&path).unwrap();
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.local_addr().unwrap());
        let (release, wait) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            wait.recv_timeout(Duration::from_secs(3)).unwrap();
            let body = r#"{"error":"fixture rejected settings"}"#;
            write!(stream, "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        let mut app = headless_app(&path);
        app.admin_base = base;
        app.status =
            Some(serde_json::from_value(serde_json::json!({"version":"test","pid":1})).unwrap());
        app.draft.bridge.call_timeout_secs = 77;
        app.save_settings();
        // The server has not replied: an inline HTTP implementation cannot
        // reach this point with a pending job and preserved editing state.
        assert!(app.settings_save.is_running());
        assert!(app.config_dirty());
        release.send(()).unwrap();
        worker.join().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(result) = app.settings_save.poll() {
                app.finish_settings_save(result);
                break;
            }
            assert!(Instant::now() < deadline, "save worker did not complete");
            std::thread::yield_now();
        }
        assert!(app
            .settings_error
            .as_deref()
            .unwrap()
            .contains("fixture rejected settings"));
        assert_eq!(app.draft.bridge.call_timeout_secs, 77);
        assert!(app.config_dirty());
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn settings_tab_renders_official_tools_rows_headlessly() {
        // Drive the REAL dashboard renderer over the Settings tab in a
        // headless egui pass — the OFFICIAL TOOLS section (incl. the
        // Linux-only wine rows) must build without panicking.
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        let mut app = headless_app(&config_path);
        app.dash_tab = dashboard::DashTab::Settings;
        app.open_settings();

        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1040.0, 2600.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                dashboard::render(&mut app, ui);
            });
        });
    }
}
