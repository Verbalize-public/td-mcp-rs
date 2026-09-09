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
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use eframe::egui;
use serde::Deserialize;
use tdmcp_config::ConfigFile;

/// Thumbnail textures kept resident. The tree only ever shows a few dozen at a
/// time, so this is generous — but it is a cap, because 281 uncapped textures
/// is real video memory held for a tab the user may never scroll.
const THUMB_CACHE_CAP: usize = 240;

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
    /// Last probe failure detail, when there was one.
    #[serde(default)]
    pub(crate) probe_message: Option<String>,
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

    /// Which dot this row gets.
    ///
    /// A wedge suspect outranks everything — "this may hang TouchDesigner" is
    /// the one fact that changes how the user treats the component. But a
    /// single failed probe no longer hides a usable card: the last probe run
    /// failing says nothing about a card that was written from earlier
    /// evidence, and painting every carded row red hides the library behind a
    /// stale ledger. Card status wins; the failed probe stays visible as a
    /// note in the detail pane.
    pub(crate) fn state(&self) -> RowState {
        if self.ignored {
            RowState::Ignored
        } else if self.probe_status == "suspect" {
            RowState::Failed
        } else if self.card_status == "stale" {
            RowState::Stale
        } else if self.card_status == "described" {
            RowState::Carded
        } else if self.probe_status == "failed" {
            RowState::Failed
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
// Scan prompt
// ---------------------------------------------------------------------------

/// What the agent scan prompt covers. The GUI runs nothing itself — it composes
/// the brief an agent executes, so this is just the prompt's inputs.
#[derive(Debug, Clone)]
pub(crate) struct ScanState {
    /// The slice as a human sentence ("Generators · undescribed").
    pub(crate) slice: String,
    /// Category the slice is limited to, when one is.
    pub(crate) category: Option<String>,
    /// A single pinned component (per-component describe button).
    pub(crate) single: Option<String>,
    /// Selector status for the slice before `force` is applied.
    pub(crate) base_status: &'static str,
    /// pid the agent should probe against; none means it spawns a throwaway.
    pub(crate) pid: Option<u32>,
    /// Regenerate cards that already exist.
    pub(crate) force: bool,
    /// Concrete throwaway path suggested when no pid is selected.
    pub(crate) throwaway: String,
}

impl ScanState {
    /// The `select` object the prompt embeds, as JSON text.
    ///
    /// `force` widens the slice to every non-ignored component in it — a card
    /// that exists is exactly the one being regenerated.
    pub(crate) fn selector(&self) -> String {
        if let Some(id) = &self.single {
            return format!("{{\"ids\": [\"{id}\"]}}");
        }
        let status = if self.force { "all" } else { self.base_status };
        match &self.category {
            Some(cat) => format!("{{\"category\": \"{cat}\", \"status\": \"{status}\"}}"),
            None => format!("{{\"status\": \"{status}\"}}"),
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
    /// A job ended; the payload is a snack line (empty = say nothing).
    Done(String),
    Failed(String),
}

/// Which job is in flight, for button gating and the status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Job {
    Loading,
    Rescanning,
}

impl Job {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Loading => "loading roster",
            Self::Rescanning => "rescanning",
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
    /// behind a running Rescan.
    pub(crate) detail_rx: Option<Receiver<Msg>>,

    pub(crate) scan_open: bool,
    pub(crate) scan: ScanState,

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
            scan_open: false,
            scan: ScanState {
                slice: String::new(),
                category: None,
                single: None,
                base_status: "undescribed",
                pid: None,
                force: false,
                throwaway: String::new(),
            },
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
    F: FnOnce(Sender<Msg>, String, ConfigFile) + Send + 'static,
{
    if app.palette.job.is_some() {
        app.snack("A palette job is already running", SnackTone::Warn);
        return false;
    }
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    app.palette.job = Some(job);
    app.palette.rx = Some(rx);
    let admin_base = app.admin_base.clone();
    let cfg = app.draft.clone();
    std::thread::spawn(move || work(tx, admin_base, cfg));
    true
}

/// Load the roster. Called on first paint of the tab and after any mutation.
pub(crate) fn load_roster(app: &mut DashboardApp) {
    start(app, Job::Loading, |tx, base, cfg| {
        send_roster(&tx, &base, &cfg);
        let _ = tx.send(Msg::Done(String::new()));
    });
}

/// Reconcile the index against disk, then reload.
pub(crate) fn rescan(app: &mut DashboardApp) {
    start(app, Job::Rescanning, |tx, base, cfg| {
        match index_call(&base, &cfg, serde_json::json!({ "action": "scan" })) {
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
        }
    });
}

/// Fetch the card behind one row.
pub(crate) fn load_detail(app: &mut DashboardApp, id: String) {
    // Detail reads are small and frequent; they ride their own thread rather
    // than the job slot so they never block a running Rescan.
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
    start(app, Job::Loading, move |tx, base, cfg| {
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
        Msg::Done(note) => {
            app.palette.job = None;
            app.palette.rx = None;
            if !note.is_empty() {
                app.snack(&note, SnackTone::Ok);
            }
        }
        Msg::Failed(e) => {
            app.palette.job = None;
            app.palette.rx = None;
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

/// The agent prompt this popup exists to produce: the describe loop for the
/// selected slice, with the TD-instance decision spelled out.
///
/// The GUI runs nothing itself — it has no LLM and no probe loop anymore. This
/// text is the whole feature; everything the agent needs must be in it, because
/// the agent never sees this screen.
pub(crate) fn scan_brief(state: &ScanState) -> String {
    let td_line = match state.pid {
        Some(pid) => format!(
            "Probe against the running TouchDesigner instance pid `{pid}`. Pass that pid to \
             every `palette_probe` call."
        ),
        None => format!(
            "No TouchDesigner instance is selected — spawn a throwaway project for probing \
             (`spawn_td` with a fresh .toe path such as `{throwaway}` and createIfMissing:true) \
             and pass that pid to every `palette_probe` call. Never probe into a project with \
             real work in it.",
            throwaway = state.throwaway,
        ),
    };
    let force_line = if state.force {
        "\nCards already exist for part of this slice — regenerate them anyway; do not skip \
         described components.\n"
    } else {
        ""
    };
    format!(
        "Run the palette-scan describe loop over {slice}.\n\
         \n\
         {td_line}\n\
         {force_line}\n\
         1. `palette_index` {{\"action\": \"scan\"}} — reconcile the roster with disk first.\n\
         2. Loop: `palette_probe` {{\"pid\": <pid from above>, \"select\": {selector}, \
         \"thumbnails\": true}} (batch default is 3), then for each digest row \
         `palette_index` {{\"action\": \"describe\", \"paletteId\": …, \"summary\": …, \
         \"tags\": […], \"body\": …}}.\n\
         3. Repeat until `palette_index` {{\"action\": \"stats\"}} shows none left in the slice.\n\
         \n\
         Card shape and the blacklist rules: tdmcp://docs/palette-scan\n",
        slice = state.slice,
        td_line = td_line,
        force_line = force_line,
        selector = state.selector(),
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
            probe_message: None,
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
    fn attention_states_outrank_card_states_on_a_row() {
        let mut r = row("builtin:Tools/wedges", "wedges", Some("summary"));
        r.probe_status = "suspect".into();
        assert_eq!(r.state(), RowState::Failed);
    }

    #[test]
    fn a_failed_probe_no_longer_hides_a_usable_card() {
        // The regression this guards: one bad probe pass wrote `failed` onto
        // every entry in the store, and the dot painted the whole library red
        // even though the cards were fine and loadable.
        let mut r = row("builtin:Generators/checker", "checker", Some("summary"));
        r.probe_status = "failed".into();
        r.probe_message = Some("loadTox produced no component".into());
        assert_eq!(r.state(), RowState::Carded);

        // Without a card the failure is still the most useful fact.
        let mut bare = row("user:Tools/mystery", "mystery", None);
        bare.probe_status = "failed".into();
        assert_eq!(bare.state(), RowState::Failed);
    }

    #[test]
    fn the_scan_brief_names_the_instance_or_the_throwaway() {
        let mut state = ScanState {
            slice: "Generators · undescribed".into(),
            category: Some("Generators".into()),
            single: None,
            base_status: "undescribed",
            pid: Some(4242),
            force: false,
            throwaway: "/tmp/palette-probe.toe".into(),
        };
        let brief = scan_brief(&state);
        assert!(brief.contains("pid `4242`"));
        assert!(brief.contains("\"category\": \"Generators\", \"status\": \"undescribed\""));
        assert!(!brief.contains("regenerate"));
        assert!(brief.contains("tdmcp://docs/palette-scan"));

        // No pid: the prompt must tell the agent to spawn a throwaway itself.
        state.pid = None;
        let brief = scan_brief(&state);
        assert!(brief.contains("spawn a throwaway project"));
        assert!(brief.contains("/tmp/palette-probe.toe"));

        // Force widens the selector past already-described components.
        state.force = true;
        let brief = scan_brief(&state);
        assert!(brief.contains("\"status\": \"all\""));
        assert!(brief.contains("regenerate them anyway"));
    }

    #[test]
    fn a_single_component_brief_pins_the_id() {
        let state = ScanState {
            slice: "component builtin:Generators/checker".into(),
            category: None,
            single: Some("builtin:Generators/checker".into()),
            base_status: "undescribed",
            pid: None,
            force: true,
            throwaway: "/tmp/palette-probe.toe".into(),
        };
        let brief = scan_brief(&state);
        // Force is meaningless for a pinned id: the selector names it exactly.
        assert!(brief.contains("{\"ids\": [\"builtin:Generators/checker\"]}"));
        assert!(!brief.contains("\"status\""));
    }
}
