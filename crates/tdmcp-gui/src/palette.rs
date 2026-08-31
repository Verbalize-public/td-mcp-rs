//! Palette section state, wire DTOs, and the background jobs that feed it.
//!
//! Drawing lives in [`crate::dashboard::palette`]; this module owns everything
//! that is not a pixel — the roster snapshot, the selection, the thumbnail
//! texture cache, and the worker threads that talk to the daemon.
//!
//! Every backend call goes through `POST /mcp/tools/call`, the same sessionless
//! JSON surface the GUI already uses for `spawn_td`. That means the roster the
//! user browses and the roster an agent queries are the *same* roster, computed
//! once by `palette_index` — the GUI never re-implements scanning, selection,
//! card status, or the blacklist.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use serde::Deserialize;
use tdmcp_config::ConfigFile;

/// Thumbnail textures kept resident. The tree only ever shows a few dozen at a
/// time, so this is generous — but it is a cap, because 281 uncapped textures
/// is real video memory held for a tab the user may never scroll.
const THUMB_CACHE_CAP: usize = 240;

/// Components probed per bridge call. Mirrors `palette::PROBE_BATCH_DEFAULT`:
/// one call is one bridge task, so a component that wedges TouchDesigner costs
/// its batch and no more.
const PROBE_BATCH: usize = 3;

/// Ceiling on probe batches in one Analyse run, so a runaway loop against a
/// misbehaving TD cannot spin forever unattended.
const MAX_BATCHES: usize = 400;

/// `palette_index list` is capped at 500 rows per page.
const LIST_PAGE: usize = 500;

// ---------------------------------------------------------------------------
// Wire DTOs — the `palette_index` row shapes, verbatim
// ---------------------------------------------------------------------------

/// One roster row as `palette_index list` returns it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PaletteRow {
    pub(crate) palette_id: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) category: String,
    #[serde(default)]
    pub(crate) source: String,
    #[serde(default)]
    pub(crate) summary: Option<String>,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    #[serde(default)]
    pub(crate) card_status: String,
    #[serde(default)]
    pub(crate) probe_status: String,
    #[serde(default)]
    pub(crate) ignored: bool,
    /// Absolute PNG path; present only when the file is really on disk.
    #[serde(default)]
    pub(crate) thumb: Option<String>,
}

impl PaletteRow {
    /// True for a component from the user's own palette folder, not the
    /// TouchDesigner install — worth marking, because their gotchas are the
    /// user's own and their card is theirs to keep current.
    pub(crate) fn is_user(&self) -> bool {
        self.source == "user"
    }

    /// Category as shown in the tree; root-level entries get a real word
    /// rather than an empty header.
    pub(crate) fn group(&self) -> &str {
        if self.category.is_empty() {
            "(root)"
        } else {
            &self.category
        }
    }

    /// Which dot this row gets. Attention states win over card states — a
    /// component that wedged TD matters more than one nobody has described.
    pub(crate) fn state(&self) -> RowState {
        if self.ignored {
            RowState::Ignored
        } else if self.probe_status == "failed" || self.probe_status == "suspect" {
            RowState::Failed
        } else if self.card_status == "stale" {
            RowState::Stale
        } else if self.card_status == "described" {
            RowState::Carded
        } else {
            RowState::Undescribed
        }
    }
}

/// Roster state of one component, in precedence order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowState {
    Carded,
    Undescribed,
    Stale,
    Failed,
    Ignored,
}

impl RowState {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Carded => "carded",
            Self::Undescribed => "undescribed",
            Self::Stale => "stale",
            Self::Failed => "failed",
            Self::Ignored => "ignored",
        }
    }
}

/// `palette_index stats` — the coverage picture under the tree.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PaletteStats {
    #[serde(default)]
    pub(crate) total: usize,
    #[serde(default)]
    pub(crate) described: usize,
    #[serde(default)]
    pub(crate) stale: usize,
    #[serde(default)]
    pub(crate) undescribed: usize,
    #[serde(default)]
    pub(crate) failed: usize,
    #[serde(default)]
    pub(crate) ignored: usize,
    #[serde(default)]
    pub(crate) scanned_at: Option<String>,
}

/// The card + paths behind the selected row (`palette_index get`).
#[derive(Debug, Clone, Default)]
pub(crate) struct PaletteDetail {
    pub(crate) palette_id: String,
    pub(crate) tox_path: String,
    pub(crate) card: Option<String>,
    pub(crate) card_error: Option<String>,
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Toolbar filter. Every variant filters on a field the **daemon** computed, so
/// "what counts as carded" is defined once, in `palette_index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum StatusFilter {
    #[default]
    All,
    Carded,
    Undescribed,
    Failed,
    Ignored,
}

impl StatusFilter {
    pub(crate) const ALL: [StatusFilter; 5] = [
        StatusFilter::All,
        StatusFilter::Carded,
        StatusFilter::Undescribed,
        StatusFilter::Failed,
        StatusFilter::Ignored,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Carded => "carded",
            Self::Undescribed => "undescribed",
            Self::Failed => "failed",
            Self::Ignored => "ignored",
        }
    }

    /// Selector `status` string for a bulk action over this slice.
    pub(crate) fn select_status(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Carded => "described",
            Self::Undescribed => "undescribed",
            Self::Failed => "failed",
            Self::Ignored => "ignored",
        }
    }

    fn accepts(self, row: &PaletteRow) -> bool {
        match self {
            // Blacklisted entries are noise in every view but their own.
            Self::All => !row.ignored,
            Self::Carded => matches!(row.state(), RowState::Carded | RowState::Stale),
            Self::Undescribed => row.state() == RowState::Undescribed,
            Self::Failed => row.state() == RowState::Failed,
            Self::Ignored => row.ignored,
        }
    }
}

// ---------------------------------------------------------------------------
// Analyse job
// ---------------------------------------------------------------------------

/// The four steps of an Analyse run, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Rescan,
    Probe,
    Thumbnails,
    Cards,
}

impl Step {
    pub(crate) const ALL: [Step; 4] = [Step::Rescan, Step::Probe, Step::Thumbnails, Step::Cards];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Rescan => "rescan",
            Self::Probe => "probe",
            Self::Thumbnails => "thumbnails",
            Self::Cards => "cards",
        }
    }
}

/// How far one step got. `Cards` never reaches `Done` from the GUI — writing a
/// card needs a language model, and pretending otherwise would be a lie told in
/// a checkmark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum StepState {
    #[default]
    Pending,
    Running,
    Done,
    /// Reached, but not something this program can finish.
    HandedOff,
    Failed,
}

/// Live progress for the Analyse modal.
#[derive(Debug, Clone)]
pub(crate) struct AnalyseState {
    pub(crate) states: [(Step, StepState, String); 4],
    pub(crate) running: bool,
    pub(crate) finished: bool,
    /// Slice the run covers, as a human sentence.
    pub(crate) slice: String,
    /// Selector the slice maps to, for the agent brief.
    pub(crate) select_status: &'static str,
    pub(crate) category: Option<String>,
    /// pid the probe ran against, once chosen.
    pub(crate) pid: Option<u32>,
    /// Undescribed components left in the slice when the run ended.
    pub(crate) undescribed_left: usize,
}

impl Default for AnalyseState {
    fn default() -> Self {
        Self::fresh(String::new(), "undescribed", None)
    }
}

impl AnalyseState {
    pub(crate) fn fresh(
        slice: String,
        select_status: &'static str,
        category: Option<String>,
    ) -> Self {
        Self {
            states: Step::ALL.map(|step| (step, StepState::Pending, String::new())),
            running: false,
            finished: false,
            slice,
            select_status,
            category,
            pid: None,
            undescribed_left: 0,
        }
    }

    pub(crate) fn set(&mut self, step: Step, state: StepState, note: impl Into<String>) {
        let note = note.into();
        if let Some(slot) = self.states.iter_mut().find(|s| s.0 == step) {
            slot.1 = state;
            slot.2 = note;
        }
    }
}

// ---------------------------------------------------------------------------
// Worker messages
// ---------------------------------------------------------------------------

pub(crate) enum Msg {
    /// A fresh roster + coverage snapshot.
    Roster(Vec<PaletteRow>, PaletteStats),
    /// The card behind one selected row.
    Detail(PaletteDetail),
    /// One Analyse step moved.
    Progress(Step, StepState, String),
    /// The pid the Analyse run settled on.
    UsingPid(u32),
    /// Undescribed components still in the slice at the end of a run.
    Remaining(usize),
    /// A job ended; the payload is a snack line (empty = say nothing).
    Done(String),
    Failed(String),
}

/// Which job is in flight, for button gating and the status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Job {
    Loading,
    Rescanning,
    Analysing,
}

impl Job {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Loading => "loading roster",
            Self::Rescanning => "rescanning",
            Self::Analysing => "analysing",
        }
    }
}

// ---------------------------------------------------------------------------
// View state
// ---------------------------------------------------------------------------

pub(crate) struct PaletteView {
    /// Full roster, ignored entries included; filtering happens on top.
    pub(crate) rows: Vec<PaletteRow>,
    pub(crate) stats: PaletteStats,
    /// True once a roster load has completed (success or empty).
    pub(crate) loaded: bool,
    pub(crate) error: Option<String>,

    pub(crate) search: String,
    pub(crate) filter: StatusFilter,
    /// Categories the user has folded away.
    pub(crate) collapsed: HashSet<String>,
    pub(crate) selected: Option<String>,
    pub(crate) detail: Option<PaletteDetail>,

    pub(crate) job: Option<Job>,
    pub(crate) rx: Option<Receiver<Msg>>,
    /// Card reads ride their own channel so opening a row never has to wait
    /// behind a running Rescan or Analyse.
    pub(crate) detail_rx: Option<Receiver<Msg>>,
    pub(crate) cancel: Arc<AtomicBool>,

    pub(crate) analyse_open: bool,
    pub(crate) analyse: AnalyseState,

    /// `paletteId` → decoded texture. `None` means "tried and could not", so a
    /// broken PNG is not re-decoded every frame.
    thumbs: HashMap<String, Option<egui::TextureHandle>>,
    thumb_order: VecDeque<String>,
}

impl Default for PaletteView {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            stats: PaletteStats::default(),
            loaded: false,
            error: None,
            search: String::new(),
            filter: StatusFilter::All,
            collapsed: HashSet::new(),
            selected: None,
            detail: None,
            job: None,
            rx: None,
            detail_rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            analyse_open: false,
            analyse: AnalyseState::default(),
            thumbs: HashMap::new(),
            thumb_order: VecDeque::new(),
        }
    }
}

impl PaletteView {
    /// Rows passing the current search + filter, sorted for the tree.
    pub(crate) fn visible_rows(&self) -> Vec<&PaletteRow> {
        let needle = self.search.trim().to_ascii_lowercase();
        let mut out: Vec<&PaletteRow> = self
            .rows
            .iter()
            .filter(|r| self.filter.accepts(r))
            .filter(|r| {
                needle.is_empty()
                    || r.palette_id.to_ascii_lowercase().contains(&needle)
                    || r.summary
                        .as_deref()
                        .is_some_and(|s| s.to_ascii_lowercase().contains(&needle))
                    || r.tags
                        .iter()
                        .any(|t| t.to_ascii_lowercase().contains(&needle))
            })
            .collect();
        out.sort_by(|a, b| {
            a.group().cmp(b.group()).then_with(|| {
                a.name
                    .to_ascii_lowercase()
                    .cmp(&b.name.to_ascii_lowercase())
            })
        });
        out
    }

    /// The selected row, if it survived the current filter.
    pub(crate) fn selected_row(&self) -> Option<&PaletteRow> {
        let id = self.selected.as_deref()?;
        self.rows.iter().find(|r| r.palette_id == id)
    }

    /// Texture for a row's thumbnail, decoded on first sight and LRU-capped.
    ///
    /// `None` means there is no usable picture — the caller draws a monogram
    /// tile instead, which is also the honest "not rendered yet" state.
    pub(crate) fn thumb(
        &mut self,
        ctx: &egui::Context,
        row: &PaletteRow,
    ) -> Option<egui::TextureHandle> {
        if let Some(hit) = self.thumbs.get(&row.palette_id) {
            return hit.clone();
        }
        let decoded = row.thumb.as_ref().and_then(|path| {
            let bytes = std::fs::read(path).ok()?;
            let icon = crate::tray::load_rgba(&bytes, None).ok()?;
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            Some(ctx.load_texture(
                format!("tdmcp_thumb_{}", row.palette_id),
                image,
                egui::TextureOptions::LINEAR,
            ))
        });
        self.thumbs.insert(row.palette_id.clone(), decoded.clone());
        self.thumb_order.push_back(row.palette_id.clone());
        while self.thumb_order.len() > THUMB_CACHE_CAP {
            if let Some(evict) = self.thumb_order.pop_front() {
                self.thumbs.remove(&evict);
            }
        }
        decoded
    }

    /// Drop every cached texture — used after a thumbnail pass replaces the
    /// files on disk, so the tree shows the new pictures rather than the old.
    pub(crate) fn forget_thumbs(&mut self) {
        self.thumbs.clear();
        self.thumb_order.clear();
    }
}

// ---------------------------------------------------------------------------
// Tool transport
// ---------------------------------------------------------------------------

/// Bearer for the local daemon's psk-gated routes (`/mcp/tools/*`).
fn bearer_of(cfg: &ConfigFile) -> Option<String> {
    if cfg.auth.mode == "psk" && !cfg.auth.psk.is_empty() {
        Some(cfg.auth.psk.clone())
    } else {
        None
    }
}

/// Call one MCP tool over the daemon's sessionless JSON route and unwrap it.
///
/// The route answers `{ok:true, data:<tool result>}` on success and a curated
/// failure envelope otherwise; both shapes are flattened to `Result` here so no
/// call site has to know about the wrapper.
fn call_tool(
    admin_base: &str,
    cfg: &ConfigFile,
    name: &str,
    arguments: serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let url = format!("{}/mcp/tools/call", admin_base.trim_end_matches('/'));
    let body = serde_json::json!({ "name": name, "arguments": arguments });
    let v = crate::http::http_post_blocking_with_timeout(
        &url,
        bearer_of(cfg).as_deref(),
        Some(&body),
        timeout,
    )?;
    let ok = v.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
    match (ok, v.get("data")) {
        (true, Some(data)) => Ok(data.clone()),
        _ => Err(v
            .get("summary")
            .or_else(|| v.get("message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("the daemon rejected the call")
            .to_owned()),
    }
}

/// `palette_index` is offline — no pid, no bridge, so it answers fast.
fn index_call(
    admin_base: &str,
    cfg: &ConfigFile,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    call_tool(
        admin_base,
        cfg,
        "palette_index",
        args,
        Duration::from_secs(30),
    )
}

/// Read the whole roster, following `truncation.nextOffset` to the end.
fn fetch_roster(admin_base: &str, cfg: &ConfigFile) -> Result<Vec<PaletteRow>, String> {
    let mut rows = Vec::new();
    let mut offset = 0usize;
    loop {
        let out = index_call(
            admin_base,
            cfg,
            serde_json::json!({
                "action": "list",
                "select": {
                    "status": "all",
                    "includeIgnored": true,
                    "limit": LIST_PAGE,
                    "offset": offset,
                },
            }),
        )?;
        let page: Vec<PaletteRow> = serde_json::from_value(
            out.get("entries")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )
        .map_err(|e| format!("unreadable roster page: {e}"))?;
        let got = page.len();
        rows.extend(page);
        match out.get("truncation").and_then(|t| t.get("nextOffset")) {
            Some(next) if got > 0 => {
                offset = next.as_u64().unwrap_or_default() as usize;
            }
            _ => break,
        }
    }
    Ok(rows)
}

fn fetch_stats(admin_base: &str, cfg: &ConfigFile) -> PaletteStats {
    index_call(admin_base, cfg, serde_json::json!({ "action": "stats" }))
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// Roster + coverage in one message. An empty index is a legitimate answer —
/// it means "nothing scanned yet", which the page renders as guidance.
fn send_roster(tx: &Sender<Msg>, admin_base: &str, cfg: &ConfigFile) {
    match fetch_roster(admin_base, cfg) {
        Ok(rows) => {
            let stats = fetch_stats(admin_base, cfg);
            let _ = tx.send(Msg::Roster(rows, stats));
        }
        Err(e) if e.contains("palette index is empty") || e.contains("not_indexed") => {
            let _ = tx.send(Msg::Roster(Vec::new(), PaletteStats::default()));
        }
        Err(e) => {
            let _ = tx.send(Msg::Failed(e));
        }
    }
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

use crate::app::{DashboardApp, SnackTone};

/// Start a job unless one is already running, wiring up a fresh channel.
fn start<F>(app: &mut DashboardApp, job: Job, work: F) -> bool
where
    F: FnOnce(Sender<Msg>, String, ConfigFile, Arc<AtomicBool>) + Send + 'static,
{
    if app.palette.job.is_some() {
        app.snack("A palette job is already running", SnackTone::Warn);
        return false;
    }
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    app.palette.job = Some(job);
    app.palette.rx = Some(rx);
    app.palette.cancel = Arc::new(AtomicBool::new(false));
    let admin_base = app.admin_base.clone();
    let cfg = app.draft.clone();
    let cancel = Arc::clone(&app.palette.cancel);
    std::thread::spawn(move || work(tx, admin_base, cfg, cancel));
    true
}

/// Load the roster. Called on first paint of the tab and after any mutation.
pub(crate) fn load_roster(app: &mut DashboardApp) {
    start(app, Job::Loading, |tx, base, cfg, _cancel| {
        send_roster(&tx, &base, &cfg);
        let _ = tx.send(Msg::Done(String::new()));
    });
}

/// Reconcile the index against disk, then reload.
pub(crate) fn rescan(app: &mut DashboardApp) {
    start(
        app,
        Job::Rescanning,
        |tx, base, cfg, _cancel| match index_call(
            &base,
            &cfg,
            serde_json::json!({ "action": "scan" }),
        ) {
            Ok(v) => {
                let note = format!(
                    "Scanned {} component(s) — {} new, {} gone",
                    v.get("total")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    v.get("added")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    v.get("removed")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                );
                send_roster(&tx, &base, &cfg);
                let _ = tx.send(Msg::Done(note));
            }
            Err(e) => {
                let _ = tx.send(Msg::Failed(e));
            }
        },
    );
}

/// Fetch the card behind one row.
pub(crate) fn load_detail(app: &mut DashboardApp, id: String) {
    // Detail reads are small and frequent; they ride their own thread rather
    // than the job slot so they never block Rescan or Analyse.
    let admin_base = app.admin_base.clone();
    let cfg = app.draft.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    app.palette.detail_rx = Some(rx);
    std::thread::spawn(move || {
        let msg = match index_call(
            &admin_base,
            &cfg,
            serde_json::json!({ "action": "get", "paletteId": id }),
        ) {
            Ok(v) => Msg::Detail(PaletteDetail {
                palette_id: v
                    .pointer("/entry/paletteId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(&id)
                    .to_owned(),
                tox_path: v
                    .get("toxPath")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                card: v
                    .get("card")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                card_error: v
                    .get("cardError")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            }),
            Err(e) => Msg::Failed(e),
        };
        let _ = tx.send(msg);
    });
}

/// Flip an entry's blacklist state, then reload the roster.
pub(crate) fn set_ignored(app: &mut DashboardApp, id: String, ignore: bool) {
    let action = if ignore { "ignore" } else { "unignore" };
    start(app, Job::Loading, move |tx, base, cfg, _cancel| {
        let out = index_call(
            &base,
            &cfg,
            serde_json::json!({ "action": action, "patterns": [id] }),
        );
        match out {
            Ok(_) => {
                send_roster(&tx, &base, &cfg);
                let _ = tx.send(Msg::Done(
                    if ignore {
                        "Blacklisted"
                    } else {
                        "Un-blacklisted"
                    }
                    .to_owned(),
                ));
            }
            Err(e) => {
                let _ = tx.send(Msg::Failed(e));
            }
        }
    });
}

/// Components a run should actually load, in tree order.
///
/// Three filters, each load-bearing:
///
/// * the slice the user is looking at (category + status), so "Analyse" means
///   what the screen says it means;
/// * never a blacklisted entry — naming an id explicitly bypasses the
///   blacklist on the daemon, so honouring it is this side's job, and those
///   are exactly the components that can wedge TouchDesigner;
/// * only what still needs evidence — no thumbnail, or never probed — so a
///   second run over the same slice is cheap instead of redundant.
fn analyse_targets(view: &PaletteView, category: Option<&str>, select_status: &str) -> Vec<String> {
    view.rows
        .iter()
        .filter(|r| !r.ignored)
        .filter(|r| category.is_none_or(|c| r.group() == c))
        .filter(|r| match select_status {
            "described" => matches!(r.state(), RowState::Carded | RowState::Stale),
            "undescribed" => r.state() == RowState::Undescribed,
            "failed" => r.state() == RowState::Failed,
            _ => true,
        })
        .filter(|r| r.thumb.is_none() || r.probe_status == "unprobed")
        .map(|r| r.palette_id.clone())
        .collect()
}

/// The mechanical half of an analysis pass: rescan, probe the slice for
/// interface evidence, render thumbnails — then stop and say plainly that the
/// cards themselves need an agent.
pub(crate) fn analyse(app: &mut DashboardApp, pid: u32) {
    let category = app.palette.analyse.category.clone();
    let select_status = app.palette.analyse.select_status;
    // The batch list is computed here, from the roster already on screen,
    // rather than left to the selector. `palette_probe`'s selection is built
    // for the *describe* loop, where each pass shrinks the slice because the
    // agent wrote a card in between; a thumbnail pass changes nothing the
    // selector looks at, so the same components would come back every batch,
    // forever. An explicit id list terminates by construction and gives the
    // modal an honest denominator.
    let targets = analyse_targets(&app.palette, category.as_deref(), select_status);

    let started = start(app, Job::Analysing, move |tx, base, cfg, cancel| {
        let _ = tx.send(Msg::UsingPid(pid));

        // 1 — rescan.
        let _ = tx.send(Msg::Progress(
            Step::Rescan,
            StepState::Running,
            String::new(),
        ));
        match index_call(&base, &cfg, serde_json::json!({ "action": "scan" })) {
            Ok(v) => {
                let note = format!(
                    "{} indexed · +{} · {} ignored",
                    v.get("total")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    v.get("added")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    v.get("ignored")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                );
                let _ = tx.send(Msg::Progress(Step::Rescan, StepState::Done, note));
            }
            Err(e) => {
                let _ = tx.send(Msg::Progress(Step::Rescan, StepState::Failed, e.clone()));
                let _ = tx.send(Msg::Failed(e));
                return;
            }
        }

        // 2 + 3 — probe the slice in small batches, thumbnails on. Probe and
        // thumbnail are one call: the component only exists between load and
        // destroy, so this is the single window either can happen in.
        let total = targets.len();
        let _ = tx.send(Msg::Progress(
            Step::Probe,
            StepState::Running,
            format!("0 / {total}"),
        ));
        let _ = tx.send(Msg::Progress(
            Step::Thumbnails,
            StepState::Running,
            String::new(),
        ));
        let (mut digested, mut failed, mut shots) = (0usize, 0usize, 0usize);
        let mut cancelled = false;
        for batch in targets.chunks(PROBE_BATCH).take(MAX_BATCHES) {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            let out = call_tool(
                &base,
                &cfg,
                "palette_probe",
                serde_json::json!({
                    "pid": pid,
                    "select": { "ids": batch },
                    "thumbnails": true,
                }),
                // A batch is one bridge task on the script timeout class, and
                // it loads three unknown components — give it real room.
                Duration::from_secs(180),
            );
            let v = match out {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.send(Msg::Progress(Step::Probe, StepState::Failed, e.clone()));
                    let _ = tx.send(Msg::Failed(e));
                    return;
                }
            };
            let rows = v
                .get("results")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            for row in &rows {
                if row.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
                    digested += 1;
                    if row.get("thumb").is_some() {
                        shots += 1;
                    }
                } else {
                    failed += 1;
                }
            }
            let _ = tx.send(Msg::Progress(
                Step::Probe,
                StepState::Running,
                format!("{} / {total}", digested + failed),
            ));
            let _ = tx.send(Msg::Progress(
                Step::Thumbnails,
                StepState::Running,
                format!("{shots} rendered"),
            ));
        }

        let probe_note = if total == 0 {
            "nothing left to probe in this slice".to_owned()
        } else {
            format!(
                "{digested} digested · {failed} failed{}",
                if cancelled { " · cancelled" } else { "" }
            )
        };
        let _ = tx.send(Msg::Progress(
            Step::Probe,
            if cancelled {
                StepState::Failed
            } else {
                StepState::Done
            },
            probe_note,
        ));
        let fallback = digested.saturating_sub(shots);
        let _ = tx.send(Msg::Progress(
            Step::Thumbnails,
            if cancelled {
                StepState::Failed
            } else {
                StepState::Done
            },
            format!(
                "{shots} rendered{}",
                if fallback > 0 {
                    // Almost always an unwrapped `.tox`: it ships no icon, and
                    // its viewer has not rasterized inside the probe's task.
                    format!(" · {fallback} drew nothing")
                } else {
                    String::new()
                }
            ),
        ));

        // 4 — the half this program cannot do.
        let stats = fetch_stats(&base, &cfg);
        let left = stats.undescribed;
        let _ = tx.send(Msg::Remaining(left));
        let _ = tx.send(Msg::Progress(
            Step::Cards,
            StepState::HandedOff,
            format!("{left} still undescribed — needs an agent"),
        ));

        send_roster(&tx, &base, &cfg);
        let _ = tx.send(Msg::Done(String::new()));
    });

    if started {
        let slice = app.palette.analyse.slice.clone();
        let cat = app.palette.analyse.category.clone();
        app.palette.analyse = AnalyseState::fresh(slice, select_status, cat);
    }
}

/// Drain worker messages into the view. Called once per frame from the tick.
pub(crate) fn poll(app: &mut DashboardApp) {
    while let Some(msg) = app
        .palette
        .detail_rx
        .as_ref()
        .and_then(|rx| rx.try_recv().ok())
    {
        apply(app, msg);
        app.palette.detail_rx = None;
    }
    loop {
        let Some(msg) = app.palette.rx.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return;
        };
        apply(app, msg);
    }
}

fn apply(app: &mut DashboardApp, msg: Msg) {
    match msg {
        Msg::Roster(rows, stats) => {
            // The files behind the old textures may have just been replaced.
            app.palette.forget_thumbs();
            app.palette.rows = rows;
            app.palette.stats = stats;
            app.palette.loaded = true;
            app.palette.error = None;
            // A selection that no longer exists must not strand the detail pane.
            if app
                .palette
                .selected
                .as_ref()
                .is_some_and(|id| !app.palette.rows.iter().any(|r| &r.palette_id == id))
            {
                app.palette.selected = None;
                app.palette.detail = None;
            }
        }
        Msg::Detail(detail) => {
            // Ignore a reply for a row the user has already navigated away from.
            if app.palette.selected.as_deref() == Some(detail.palette_id.as_str()) {
                app.palette.detail = Some(detail);
            }
        }
        Msg::Progress(step, state, note) => app.palette.analyse.set(step, state, note),
        Msg::UsingPid(pid) => app.palette.analyse.pid = Some(pid),
        Msg::Remaining(n) => app.palette.analyse.undescribed_left = n,
        Msg::Done(note) => {
            app.palette.job = None;
            app.palette.rx = None;
            if app.palette.analyse.running {
                app.palette.analyse.running = false;
                app.palette.analyse.finished = true;
            }
            if !note.is_empty() {
                app.snack(&note, SnackTone::Ok);
            }
        }
        Msg::Failed(e) => {
            app.palette.job = None;
            app.palette.rx = None;
            app.palette.analyse.running = false;
            app.palette.analyse.finished = true;
            app.palette.error = Some(e.clone());
            app.palette.loaded = true;
            app.snack(&crate::wire::clip_line(&e, 70), SnackTone::Error);
        }
    }
}

// ---------------------------------------------------------------------------
// Clipboard briefs
// ---------------------------------------------------------------------------

/// Pins line pulled out of a card body, when it has one.
///
/// Cards are written to a fixed template (`tdmcp://docs/palette-scan`), and its
/// `**Pins:**` line is the one fact a reader needs before wiring — so it is
/// worth lifting into the brief rather than making the agent read the card.
fn pins_line(card: Option<&str>) -> Option<String> {
    let body = card?;
    let raw = body
        .lines()
        .find(|l| l.trim_start().starts_with("**Pins:**"))?;
    let text = raw.trim().trim_start_matches("**Pins:**").trim();
    if text.is_empty() {
        None
    } else {
        Some(text.replace('`', ""))
    }
}

/// A leaf path suggestion for the `place` step — the component's own name,
/// sanitized to something TouchDesigner will accept without renaming it.
fn suggested_leaf(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "component".to_owned()
    } else if trimmed.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        // TD will not accept a leading digit; it would rename and lint.
        format!("p_{trimmed}")
    } else {
        trimmed.to_owned()
    }
}

/// The `mutate_nodes` step that places this component, as an agent would write it.
pub(crate) fn place_step(row: &PaletteRow) -> String {
    format!(
        "{{\"op\": \"place\", \"path\": \"/project1/{leaf}\",\n \"paletteId\": \"{id}\",\n \"comment\": \"stock {name} from the palette\"}}",
        leaf = suggested_leaf(&row.name),
        id = row.palette_id,
        name = row.name,
    )
}

/// The full reference an agent can act on without a lookup round-trip.
///
/// Everything below the first line is optional and simply absent for an
/// undescribed component — the id and the `place` step alone are already
/// enough to use it, which is the point.
pub(crate) fn reference_brief(row: &PaletteRow, detail: Option<&PaletteDetail>) -> String {
    let mut out = format!("Use palette component `{}`\n", row.palette_id);

    let mut facts = vec![row.group().to_owned(), row.state().word().to_owned()];
    if let Some(pins) = pins_line(detail.and_then(|d| d.card.as_deref())) {
        facts.push(pins);
    }
    out.push_str(&format!("({})\n", facts.join(" · ")));

    if let Some(summary) = &row.summary {
        out.push('\n');
        out.push_str(summary);
        out.push('\n');
    }

    out.push_str("\n```json\n");
    out.push_str(&place_step(row));
    out.push_str("\n```\n");

    if row.summary.is_none() {
        // Say what is missing rather than let the agent assume the palette
        // knows more about this component than it does.
        out.push_str(
            "\nNo card has been written for this component yet — `inspect` it after placing, \
             or describe it first (tdmcp://docs/palette-scan).\n",
        );
    }
    out
}

/// The hand-off brief for the half the GUI cannot do: writing the cards.
pub(crate) fn analyse_brief(state: &AnalyseState) -> String {
    let selector = match &state.category {
        Some(cat) => format!("{{\"category\": \"{cat}\", \"status\": \"undescribed\"}}"),
        None => "{\"status\": \"undescribed\"}".to_owned(),
    };
    format!(
        "Run the palette-scan describe loop over {slice} ({left} undescribed).\n\
         The evidence pass is already done — the roster is scanned and these components \
         have been probed, so go straight to writing cards.\n\n\
         1. `palette_probe` {{\"pid\": <a throwaway TD>, \"select\": {selector}, \"limit\": 3}}\n\
         2. For each digest, `palette_index` {{\"action\": \"describe\", \"paletteId\": …, \
         \"summary\": …, \"tags\": […], \"body\": …}}\n\
         3. Repeat until `palette_index` {{\"action\": \"stats\"}} shows none left in the slice.\n\n\
         Card shape and the blacklist rules: tdmcp://docs/palette-scan\n",
        slice = state.slice,
        left = state.undescribed_left,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "unit tests")]
mod tests {
    use super::*;

    fn row(id: &str, name: &str, summary: Option<&str>) -> PaletteRow {
        PaletteRow {
            palette_id: id.to_owned(),
            name: name.to_owned(),
            category: "ImageFilters".to_owned(),
            source: "builtin".to_owned(),
            summary: summary.map(str::to_owned),
            tags: Vec::new(),
            card_status: if summary.is_some() {
                "described".into()
            } else {
                "undescribed".into()
            },
            probe_status: "ok".to_owned(),
            ignored: false,
            thumb: None,
        }
    }

    #[test]
    fn a_carded_reference_carries_everything_needed_to_act() {
        let r = row(
            "builtin:ImageFilters/bloom",
            "bloom",
            Some("Classic bloom."),
        );
        let detail = PaletteDetail {
            card: Some("# bloom\n\n**Pins:** `in1` TOP · `in2` TOP → `out1` TOP\n".into()),
            ..Default::default()
        };
        let brief = reference_brief(&r, Some(&detail));
        assert!(brief.contains("builtin:ImageFilters/bloom"));
        assert!(brief.contains("in1 TOP · in2 TOP → out1 TOP"));
        assert!(brief.contains("Classic bloom."));
        assert!(brief.contains("\"op\": \"place\""));
        assert!(!brief.contains("No card has been written"));
    }

    #[test]
    fn an_undescribed_reference_is_still_actionable_and_says_what_is_missing() {
        let r = row("builtin:Tools/chromaKey", "chromaKey", None);
        let brief = reference_brief(&r, None);
        // The place step is the point: it works with or without a card.
        assert!(brief.contains("\"paletteId\": \"builtin:Tools/chromaKey\""));
        assert!(brief.contains("No card has been written"));
    }

    #[test]
    fn a_suggested_leaf_is_something_touchdesigner_will_accept() {
        assert_eq!(suggested_leaf("bloom"), "bloom");
        assert_eq!(suggested_leaf("Basic Widgets"), "Basic_Widgets");
        // A leading digit would make TD rename the node and emit a lint.
        assert_eq!(suggested_leaf("3DScope"), "p_3DScope");
        assert_eq!(suggested_leaf("---"), "component");
    }

    #[test]
    fn pins_are_only_lifted_when_the_card_actually_has_them() {
        assert!(pins_line(None).is_none());
        assert!(pins_line(Some("# x\n\nno pins here\n")).is_none());
        assert_eq!(
            pins_line(Some("**Pins:** `in1` → `out1`")).unwrap(),
            "in1 → out1"
        );
    }

    #[test]
    fn the_filter_hides_blacklisted_entries_everywhere_but_their_own_view() {
        let mut r = row("builtin:TDAbleton/x", "x", None);
        r.ignored = true;
        assert!(!StatusFilter::All.accepts(&r));
        assert!(!StatusFilter::Undescribed.accepts(&r));
        assert!(StatusFilter::Ignored.accepts(&r));
    }

    #[test]
    fn analyse_targets_are_finite_and_skip_the_blacklist() {
        // The bug this guards: `palette_probe`'s selector does not shrink when
        // a thumbnail is rendered, so a selector-driven loop re-probes the same
        // components forever. Targets are a finite list computed here instead.
        let mut view = PaletteView::default();
        let mut done = row("builtin:ImageFilters/bloom", "bloom", Some("s"));
        done.thumb = Some("/thumbs/bloom.png".into());
        done.probe_status = "ok".into();
        let todo = row("builtin:ImageFilters/chromaKey", "chromaKey", None);
        let mut hostile = row("builtin:TDAbleton/pkg", "pkg", None);
        hostile.ignored = true;
        hostile.category = "ImageFilters".into();
        let mut elsewhere = row("builtin:Tools/logger", "logger", None);
        elsewhere.category = "Tools".into();
        view.rows = vec![done, todo, hostile, elsewhere];

        let targets = analyse_targets(&view, Some("ImageFilters"), "all");
        // Only the one that still needs evidence, from the asked-for category,
        // and never the blacklisted entry — explicit ids bypass the daemon's
        // blacklist, so dropping it here is the only thing that honours it.
        assert_eq!(targets, vec!["builtin:ImageFilters/chromaKey".to_owned()]);

        // A second run over a fully-covered slice asks for nothing at all.
        let mut covered = PaletteView::default();
        let mut r = row("builtin:ImageFilters/bloom", "bloom", Some("s"));
        r.thumb = Some("/thumbs/bloom.png".into());
        r.probe_status = "ok".into();
        covered.rows = vec![r];
        assert!(analyse_targets(&covered, None, "all").is_empty());
    }

    #[test]
    fn attention_states_outrank_card_states_on_a_row() {
        let mut r = row("builtin:Tools/wedges", "wedges", Some("summary"));
        r.probe_status = "suspect".into();
        assert_eq!(r.state(), RowState::Failed);
    }
}
