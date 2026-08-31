//! Dev-only preview harness (feature `preview`): renders the real dashboard
//! from injected fixtures so every state — populated fleets, modals, banners,
//! dirty settings — can be pixel-verified without a live daemon or TD.
//!
//! Scenes: `overview-empty · overview-populated · overview-offline ·
//! modal-add-slave · stop-confirm · logs-filtered · settings-dirty ·
//! palette-tree · palette-empty · palette-analyse`.
//! Window title matches the production dashboard so the Win32 capture script
//! (`.ua/gui-shot.ps1` technique) finds it.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use eframe::egui;
use serde_json::json;

use crate::app::{DashboardApp, FleetPanel};
use crate::wire::{LogRecordView, ScanHit, ScanPurpose};

/// Run one scene in a native window; blocks until closed.
pub fn run(scene: &str) -> anyhow::Result<()> {
    let app = Box::new(build(scene)?);
    // `popup-*` scenes render the tray glance card at its real size instead of
    // the dashboard, so the footer actions can be verified without a daemon.
    let popup = scene.starts_with("popup");
    let (title, size) = if popup {
        ("td-mcp-rs", [crate::theme::WINDOW_WIDTH, 304.0])
    } else if scene == "overview-narrow" {
        ("td-mcp-rs — Dashboard", [800.0, 620.0])
    } else {
        ("td-mcp-rs — Dashboard", [1040.0, 760.0])
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size(size),
        ..Default::default()
    };
    eframe::run_native(
        "tdmcp-preview",
        options,
        Box::new(move |cc| {
            crate::theme::apply(&cc.egui_ctx);
            Ok(Box::new(PreviewApp { inner: app, popup }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))
}

struct PreviewApp {
    inner: Box<DashboardApp>,
    popup: bool,
}

impl eframe::App for PreviewApp {
    // eframe 0.35 drives apps through `ui`, matching the production popup path.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
        if self.popup {
            let app = self.inner.as_mut();
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::NONE
                        .fill(crate::theme::BG_WINDOW)
                        .stroke(egui::Stroke::new(1.0, crate::theme::BORDER))
                        .inner_margin(0.0),
                )
                .show(ui, |ui| {
                    app.draw_header(ui);
                    egui::ScrollArea::vertical()
                        .auto_shrink(false)
                        .max_height((ui.available_height() - crate::popup::FOOTER_BLOCK_H).max(0.0))
                        .show(ui, |ui| {
                            ui.add_space(crate::theme::sp::SM);
                            app.draw_summary(ui);
                        });
                    app.draw_action_footer(ui);
                });
            return;
        }
        crate::dashboard::render(self.inner.as_mut(), ui);
    }
}

fn build(scene: &str) -> anyhow::Result<DashboardApp> {
    let tmp = std::env::temp_dir();
    let quit = Arc::new(AtomicBool::new(false));
    let blank_icon = crate::tray::RgbaIcon {
        rgba: vec![0, 0, 0, 255],
        width: 1,
        height: 1,
    };
    let mut app = DashboardApp::new(
        "http://127.0.0.1:9860".to_owned(),
        tmp.clone(),
        blank_icon,
        crate::tray::RgbaIcon {
            rgba: vec![0, 0, 0, 255],
            width: 1,
            height: 1,
        },
        quit,
        tmp.join("tdmcp-preview-config.toml"),
        egui::IconData {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        },
    )?;
    // Suppress tray + initial hide + OS toasts paths.
    app.pending_tray = false;
    app.pending_initial_hide = false;
    app.visible = true;
    app.dashboard_open = true;

    match scene {
        "overview-empty" => {
            inject_status(&mut app, "master", 0, 9860);
            app.fleet_json = r#"{"processes":[]}"#.to_owned();
            app.sessions_json = r#"{"sessions":[]}"#.to_owned();
            app.apply_fleet_status();
        }
        "overview-populated" | "modal-add-slave" | "stop-confirm" | "popup"
        | "popup-stop-confirm" | "overview-narrow" | "overview-many" => {
            inject_status(&mut app, "master", 3, 9860);
            app.fleet_json = json!({
                "processes": [
                    {"pid": 12045, "title": "tox-scene-01", "bridge": "connected",
                     "tasks": [{"id": 1}, {"id": 2}], "cancelledTasks": []},
                    {"pid": 12100, "title": "tox-render", "bridge": "disconnected",
                     "tasks": null, "cancelledTasks": [{"id": 9}]},
                    {"pid": 22001, "title": "tox-studio", "bridge": "connected",
                     "tasks": [{"id": 4}], "cancelledTasks": [],
                     "hostname": "studio-b", "daemonId": SLAVE_ID}
                ]
            })
            .to_string();
            if scene == "overview-many" {
                let procs: Vec<_> = (0..7)
                    .map(|i| {
                        json!({"pid": 13000 + i, "title": format!("tox-{i:02}"),
                               "bridge": "connected", "tasks": [], "cancelledTasks": []})
                    })
                    .collect();
                app.fleet_json = json!({ "processes": procs }).to_string();
            }
            app.sessions_json = sessions_fixture();
            app.slaves_json = slaves_fixture();
            app.error_ring
                .push("bridge disconnected — pid 12100 lost IPC".to_owned());
            app.error_ring
                .push("2 task(s) cancelled on bridge loss".to_owned());
            app.crash_count = 1;
            app.apply_fleet_status();
            if scene == "modal-add-slave" {
                app.fleet_panel = FleetPanel::AddSlave;
                app.add_slave_host = "192.168.1.50".to_owned();
                app.add_slave_probe =
                    Some(serde_json::from_str(PROBE_FIXTURE).map_err(anyhow::Error::msg)?);
                app.scan_results = scan_hits();
                app.scan_purpose = ScanPurpose::AddSlave;
            }
            if scene == "stop-confirm" || scene == "popup-stop-confirm" {
                app.confirm_stop = true;
            }
        }
        "overview-offline" => {
            app.status = None;
            app.error = Some("connection refused (os error 111)".to_owned());
        }
        "logs-filtered" => {
            app.dash_tab = crate::dashboard::DashTab::Logs;
            inject_status(&mut app, "standalone", 1, 9860);
            app.logs_view.buf = log_records().into();
            app.logs_view.min_level = Some("warn");
            app.logs_view.last_fetch = Some(std::time::Instant::now());
        }
        "settings-dirty" => {
            app.dash_tab = crate::dashboard::DashTab::Settings;
            inject_status(&mut app, "master", 2, 9860);
            app.fleet_json = r#"{"processes":[]}"#.to_owned();
            app.sessions_json = r#"{"sessions":[]}"#.to_owned();
            app.draft.server.port += 7;
            app.needs_restart = true;
        }
        "palette-tree" | "palette-analyse" => {
            app.dash_tab = crate::dashboard::DashTab::Palette;
            inject_status(&mut app, "standalone", 1, 9860);
            app.fleet_json = json!({
                "processes": [
                    {"pid": 12045, "title": "palette_probe", "bridge": "connected",
                     "tasks": [], "cancelledTasks": []}
                ]
            })
            .to_string();
            app.sessions_json = r#"{"sessions":[]}"#.to_owned();
            palette_fixture(&mut app);
            if scene == "palette-analyse" {
                app.palette.analyse = crate::palette::AnalyseState::fresh(
                    "ImageFilters · undescribed".to_owned(),
                    "undescribed",
                    Some("ImageFilters".to_owned()),
                );
                app.palette.analyse.pid = Some(12045);
                app.palette.analyse.undescribed_left = 38;
                app.palette.analyse.finished = true;
                app.palette.analyse.running = false;
                use crate::palette::{Step, StepState};
                app.palette
                    .analyse
                    .set(Step::Rescan, StepState::Done, "281 indexed · +0 · 78 ignored");
                app.palette
                    .analyse
                    .set(Step::Probe, StepState::Done, "38 digested · 2 failed");
                app.palette.analyse.set(
                    Step::Thumbnails,
                    StepState::Done,
                    "36 rendered · 2 without a picture",
                );
                app.palette.analyse.set(
                    Step::Cards,
                    StepState::HandedOff,
                    "38 still undescribed — needs an agent",
                );
                app.palette.analyse_open = true;
            }
        }
        "palette-empty" => {
            app.dash_tab = crate::dashboard::DashTab::Palette;
            inject_status(&mut app, "standalone", 0, 9860);
            app.fleet_json = r#"{"processes":[]}"#.to_owned();
            app.sessions_json = r#"{"sessions":[]}"#.to_owned();
            // Loaded, but genuinely empty — the "nothing scanned yet" state.
            app.palette.loaded = true;
        }
        other => anyhow::bail!(
            "unknown scene `{other}` — expected popup · popup-stop-confirm · overview-narrow · overview-many · overview-empty · overview-populated · \
             overview-offline · modal-add-slave · stop-confirm · logs-filtered · settings-dirty · \
             palette-tree · palette-empty · palette-analyse"
        ),
    }
    Ok(app)
}

const LOCAL_ID: &str = "d4f0a2c1-77bb-4c11-9f2e-a1b2c3d4e5f6";
const SLAVE_ID: &str = "91ac33dd-02ea-49ab-8d10-9988aabbccdd";

fn inject_status(app: &mut DashboardApp, role: &str, mcp: usize, _port: u16) {
    let body = json!({
        "ok": true,
        "version": "0.1.4",
        "pid": std::process::id(),
        "mcpSessionCount": mcp,
        "bridgeCount": 2,
        "noGui": false,
        "bindAddress": "127.0.0.1",
        "role": role,
        "daemonId": LOCAL_ID,
        "hostname": "DESKTOP-A",
        "slaveCount": if role == "master" { Some(1) } else { None },
        "uptimeSecs": 3 * 3600 + 12 * 60,
    });
    app.status = serde_json::from_str(&body.to_string()).ok();
}

fn sessions_fixture() -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    json!({
        "sessions": [
            {"id": "aa11bb22-cc33-4444-5555-6666777788889999", "clientName": "claude-code",
             "clientVersion": "1.4.2", "connectedAt": now_ms - 720_000},
            {"id": "b222c333-d444-4555-6666-7777888899990000", "clientName": "cursor",
             "clientVersion": "0.42.0", "connectedAt": now_ms - 90_000},
            {"id": "c333d444-e555-4666-7777-888899990000aaaa1111", "clientName": "codex-cli",
             "clientVersion": "", "connectedAt": now_ms - 15_000}
        ]
    })
    .to_string()
}

fn slaves_fixture() -> String {
    json!({
        "slaves": [
            {"daemonId": SLAVE_ID, "hostname": "studio-b", "version": "0.1.3",
             "baseUrl": "http://192.168.1.50:9860", "authToken": "",
             "reachability": "reachable", "processCount": 3}
        ]
    })
    .to_string()
}

const PROBE_FIXTURE: &str = r#"{
    "role": "slave",
    "version": "0.1.3",
    "hostname": "studio-c",
    "daemonId": "55667788-aabb-4cdd-8eef-001122334455"
}"#;

fn scan_hits() -> Vec<ScanHit> {
    vec![
        ScanHit {
            host: "192.168.1.50".to_owned(),
            port: 9860,
            role: "slave".to_owned(),
            hostname: "studio-b".to_owned(),
            daemon_id: SLAVE_ID.to_owned(),
            version: "0.1.3".to_owned(),
        },
        ScanHit {
            host: "192.168.1.62".to_owned(),
            port: 9860,
            role: "slave".to_owned(),
            hostname: "studio-c".to_owned(),
            daemon_id: "55667788-aabb-4cdd-8eef-001122334455".to_owned(),
            version: "0.1.3".to_owned(),
        },
    ]
}

fn log_records() -> Vec<LogRecordView> {
    let mut v = Vec::new();
    for i in 0..24u64 {
        let (level, src, target, msg) = match i % 6 {
            0 => (
                "error",
                "bridge",
                "tdmcp_bridge::ipc",
                "heartbeat pong timeout after 120s",
            ),
            1 => (
                "warn",
                "daemon",
                "tdmcp_daemon::middleware",
                "session chill engaged for 250ms",
            ),
            2 => (
                "info",
                "daemon",
                "tdmcp_daemon",
                "poll ok · fleet 3 processes",
            ),
            3 => (
                "debug",
                "proxy",
                "tdmcp_proxy::stdio",
                "forwarded tools/list",
            ),
            4 => (
                "info",
                "bridge",
                "tdmcp_bridge",
                "execute_python ok pid=12045",
            ),
            _ => ("trace", "daemon", "tdmcp_daemon::ring", "tick"),
        };
        v.push(LogRecordView {
            seq: 100 + i,
            ts: format!("2026-01-01T12:{:02}:{:02}.123Z", i / 60, i % 60),
            level: level.to_owned(),
            src: src.to_owned(),
            pid: 12045,
            target: target.to_owned(),
            msg: msg.to_owned(),
            code: if level == "error" {
                // Real registered code — the diagnostics catalog scan forbids
                // unregistered tdmcp.* literals outside #[cfg(test)].
                Some("tdmcp.bridge.timeout".to_owned())
            } else {
                None
            },
            kvs: Default::default(),
        });
    }
    v
}

/// A believable palette roster: real category names and counts from the stock
/// TouchDesigner palette, four carded entries, a blacklisted family, and a
/// wedge suspect — so every row state the tree can paint is on screen at once.
///
/// Thumbnails are written as real PNGs into a temp dir and referenced by path,
/// exercising the same decode-and-cache path a probed thumbnail takes rather
/// than a shortcut only the fixture would use.
fn palette_fixture(app: &mut DashboardApp) {
    use crate::palette::{PaletteRow, PaletteStats};

    let thumb_dir = std::env::temp_dir().join("tdmcp-preview-thumbs");
    let _ = std::fs::create_dir_all(&thumb_dir);

    let mut rows: Vec<PaletteRow> = Vec::new();
    let mut push = |id: &str,
                    name: &str,
                    category: &str,
                    source: &str,
                    summary: Option<&str>,
                    card_status: &str,
                    probe_status: &str,
                    ignored: bool,
                    thumb: Option<String>| {
        rows.push(PaletteRow {
            palette_id: id.to_owned(),
            name: name.to_owned(),
            category: category.to_owned(),
            source: source.to_owned(),
            summary: summary.map(str::to_owned),
            tags: if summary.is_some() {
                vec!["image".to_owned(), "post".to_owned()]
            } else {
                Vec::new()
            },
            card_status: card_status.to_owned(),
            probe_status: probe_status.to_owned(),
            ignored,
            thumb,
        });
    };

    let carded: [(&str, &str, &str); 4] = [
        ("bloom", "ImageFilters", "Classic bloom — thresholds the bright parts of an image, blurs them, and composites the glow back."),
        ("cartesianToPolar", "ImageFilters", "Remaps an image between cartesian and polar space; the basis of kaleidoscope and radial-smear looks."),
        ("changeColor", "ImageFilters", "Selective hue replacement — pick a source colour and drive it to a target."),
        ("particlesGpu", "Tools", "GPU particle system driven by a source TOP; instanced geometry out."),
    ];
    for (i, (name, cat, summary)) in carded.iter().enumerate() {
        let thumb = write_fixture_thumb(&thumb_dir, name, i);
        push(
            &format!("builtin:{cat}/{name}"),
            name,
            cat,
            "builtin",
            Some(summary),
            "described",
            "ok",
            false,
            thumb,
        );
    }

    // The bulk of a real roster: indexed, named, and nothing more.
    let plain: [(&str, &str); 10] = [
        ("chromaKey", "ImageFilters"),
        ("edgeGlow", "ImageFilters"),
        ("checker", "Generators"),
        ("julia", "Generators"),
        ("mandelbrot", "Generators"),
        ("audioAnalysis", "Tools"),
        ("moviePlayer", "Tools"),
        ("opBrowser", "Tools"),
        ("buttons", "UI/Basic Widgets"),
        ("sliders", "UI/Basic Widgets"),
    ];
    for (name, cat) in plain {
        push(
            &format!("builtin:{cat}/{name}"),
            name,
            cat,
            "builtin",
            None,
            "undescribed",
            "unprobed",
            false,
            None,
        );
    }

    // A card written against a .tox that has since changed.
    push(
        "builtin:Techniques/instancing",
        "instancing",
        "Techniques",
        "builtin",
        Some("Worked instancing setup — geometry driven by a CHOP of transforms."),
        "stale",
        "ok",
        false,
        None,
    );
    // The one state that earns the sidebar's attention pill.
    push(
        "builtin:Techniques/SICK/scanner",
        "scanner",
        "Techniques/SICK",
        "builtin",
        None,
        "undescribed",
        "suspect",
        false,
        None,
    );
    // Blacklisted family — hidden under every filter but `ignored`.
    push(
        "builtin:TDAbleton/TDAbletonPackage",
        "TDAbletonPackage",
        "TDAbleton",
        "builtin",
        None,
        "undescribed",
        "unprobed",
        true,
        None,
    );
    // The user's own component, which gets the `yours` badge.
    push(
        "user:MyRig/projector",
        "projector",
        "MyRig",
        "user",
        Some("Projector calibration rig — keystone, blend and test patterns."),
        "described",
        "ok",
        false,
        None,
    );

    app.palette.rows = rows;
    app.palette.loaded = true;
    app.palette.stats = PaletteStats {
        total: 281,
        described: 4,
        stale: 1,
        undescribed: 197,
        failed: 1,
        ignored: 78,
        scanned_at: Some("2026-08-31T17:47:02Z".to_owned()),
    };
    app.palette.selected = Some("builtin:ImageFilters/bloom".to_owned());
    app.palette.detail = Some(crate::palette::PaletteDetail {
        palette_id: "builtin:ImageFilters/bloom".to_owned(),
        tox_path: "/Applications/TouchDesigner.app/Contents/Resources/tfs/Samples/Palette/ImageFilters/bloom.tox".to_owned(),
        card: Some(BLOOM_CARD.to_owned()),
        card_error: None,
    });
}

/// A small distinct PNG per fixture entry, so the tree shows real decoded
/// textures rather than the monogram placeholder.
fn write_fixture_thumb(dir: &std::path::Path, name: &str, seed: usize) -> Option<String> {
    const SIZE: u32 = 96;
    let mut img = image::RgbaImage::new(SIZE, SIZE);
    let (r0, g0, b0) = [
        (0xff_u8, 0x7a_u8, 0x1a_u8),
        (0x5f, 0xd3, 0x8f),
        (0x6a, 0x8f, 0xe0),
        (0xd8, 0x6a, 0xc8),
    ][seed % 4];
    for (x, y, px) in img.enumerate_pixels_mut() {
        let fx = x as f32 / SIZE as f32;
        let fy = y as f32 / SIZE as f32;
        let glow =
            ((1.0 - ((fx - 0.5).powi(2) + (fy - 0.5).powi(2)).sqrt() * 1.9).max(0.0)).powf(1.6);
        *px = image::Rgba([
            (0x18 as f32 + r0 as f32 * glow) as u8,
            (0x18 as f32 + g0 as f32 * glow) as u8,
            (0x1c as f32 + b0 as f32 * glow) as u8,
            255,
        ]);
    }
    let path = dir.join(format!("{name}.png"));
    img.save(&path).ok()?;
    Some(path.to_string_lossy().into_owned())
}

const BLOOM_CARD: &str = r#"# bloom

**What:** Classic bloom — thresholds the bright parts of an image, blurs them, and composites the glow back.
**When:** Any glow/bleed pass on a rendered or video TOP. Cheaper and more controllable than hand-wiring threshold+blur+comp.

**Pins:** `in1` TOP source · `in2` TOP (optional mask) → `out1` TOP graded result
**Key pars:** page `Bloom` (20 pars) — threshold, blur size, intensity, and the composite mode. `About` page carries Help/Version.

```opsketch
scope: bloom (COMP:baseCOMP, pars: Bloom page) nodes=29
bloom baseCOMP [custom]  # builtin:ImageFilters/bloom — threshold + blur + composite glow
  in1 inTOP
  in2 inTOP
  out1 outTOP
```

**Gotchas:** cost scales with blur size and input resolution; it is a full post pass, so put it after the look chain, not inside a feedback loop.
"#;
