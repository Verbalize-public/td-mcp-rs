//! `palette_index` — the offline half of palette awareness.
//!
//! Scans the TouchDesigner Palette (`.tox` component library) into a local
//! index, serves it back as cheap one-line rows or full cards, and owns the
//! probe blacklist. No `pid`, no bridge, session-gate exempt — same class as
//! `td_installs` / `dialogs`.
//!
//! Cards are **authored by the agent** from `palette_probe` evidence; this tool
//! only stores and serves them. See `tdmcp://docs/palette-scan`.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use tdmcp_config::ConfigFile;
use tdmcp_core::LenientU32;
use tdmcp_diagnostics::codes;
use tdmcp_projectio::palette::{
    scan_into, CardStatus, PaletteEntry, PaletteIndex, PaletteRoot, PaletteSource, PaletteStore,
    SelectStatus, Selector, AUTO_IGNORE_AFTER, LIST_LIMIT_DEFAULT, LIST_LIMIT_MAX,
};
use tdmcp_projectio::resolve;
use tdmcp_projectio::ProjectIoError;

/// Args for `palette_index`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaletteIndexParams {
    /// What to do.
    pub action: PaletteAction,
    /// Which entries the action applies to (`list` / `forget`).
    #[serde(default)]
    pub select: Option<PaletteSelectorArg>,
    /// Target id for `get` / `describe`.
    #[serde(default)]
    pub palette_id: Option<String>,
    /// One-line summary written into the index (`describe`). The retrieval
    /// surface `list` shows — say what the component does, in search words.
    #[serde(default)]
    pub summary: Option<String>,
    /// Retrieval tags (`describe`).
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Card body Markdown (`describe`).
    #[serde(default)]
    pub body: Option<String>,
    /// Id globs to add/remove from the blacklist (`ignore` / `unignore`).
    #[serde(default)]
    pub patterns: Option<Vec<String>>,
    /// Extra palette roots to scan on top of the discovered ones (`scan`).
    #[serde(default)]
    pub roots: Option<Vec<PaletteRootArg>>,
}

/// `palette_index` sub-actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PaletteAction {
    /// Walk the palette roots and reconcile the index with disk.
    Scan,
    /// One line per entry: id, category, summary, card status.
    List,
    /// Full entry plus its card body.
    Get,
    /// Write a card (summary + tags + body) for one component.
    Describe,
    /// Add id globs to the probe blacklist.
    Ignore,
    /// Remove id globs from the probe blacklist.
    Unignore,
    /// Drop entries and their cards from the index.
    Forget,
    /// Roster counts by card status / category.
    Stats,
}

/// An extra palette root supplied by the caller.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaletteRootArg {
    /// Which half of the palette this root provides.
    pub source: PaletteSourceArg,
    /// Absolute folder path.
    pub path: String,
}

/// Wire form of [`PaletteSource`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PaletteSourceArg {
    /// Shipped with the TouchDesigner install.
    Builtin,
    /// The user's own palette folder.
    User,
}

impl From<PaletteSourceArg> for PaletteSource {
    fn from(v: PaletteSourceArg) -> Self {
        match v {
            PaletteSourceArg::Builtin => Self::Builtin,
            PaletteSourceArg::User => Self::User,
        }
    }
}

/// Card / probe status filter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PaletteStatusArg {
    /// No status filter.
    #[default]
    All,
    /// No card yet.
    Undescribed,
    /// Card present and current.
    Described,
    /// Card present but the `.tox` changed under it.
    Stale,
    /// Last probe failed, or the entry is a wedge suspect.
    Failed,
    /// On the probe blacklist.
    Ignored,
}

impl From<PaletteStatusArg> for SelectStatus {
    fn from(v: PaletteStatusArg) -> Self {
        match v {
            PaletteStatusArg::All => Self::All,
            PaletteStatusArg::Undescribed => Self::Undescribed,
            PaletteStatusArg::Described => Self::Described,
            PaletteStatusArg::Stale => Self::Stale,
            PaletteStatusArg::Failed => Self::Failed,
            PaletteStatusArg::Ignored => Self::Ignored,
        }
    }
}

/// Which entries a bulk action applies to. Shared by `palette_index` and
/// `palette_probe` so partial-palette selection is defined once.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaletteSelectorArg {
    /// Exact ids. Explicit ids bypass the blacklist — asking is deliberate.
    #[serde(default)]
    pub ids: Option<Vec<String>>,
    /// Category prefix; `Tools` also matches `Tools/Sub`.
    #[serde(default)]
    pub category: Option<String>,
    /// Restrict to one root.
    #[serde(default)]
    pub source: Option<PaletteSourceArg>,
    /// `*` / `?` glob over the id.
    #[serde(default, rename = "match")]
    pub pattern: Option<String>,
    /// Card / probe status.
    #[serde(default)]
    pub status: PaletteStatusArg,
    /// Include blacklisted entries.
    #[serde(default)]
    pub include_ignored: bool,
    /// Max entries returned / probed.
    #[serde(default)]
    pub limit: Option<LenientU32>,
    /// Skip this many matches first.
    #[serde(default)]
    pub offset: Option<LenientU32>,
}

impl PaletteSelectorArg {
    /// Domain selector (paging is applied separately by the caller).
    #[must_use]
    pub fn to_selector(&self) -> Selector {
        Selector {
            ids: self.ids.clone().unwrap_or_default(),
            category: self.category.clone(),
            source: self.source.map(Into::into),
            pattern: self.pattern.clone(),
            status: self.status.into(),
            include_ignored: self.include_ignored,
        }
    }

    /// `(offset, limit)` clamped to [`LIST_LIMIT_MAX`].
    #[must_use]
    pub fn page(&self, default_limit: usize) -> (usize, usize) {
        let offset = self.offset.map_or(0, |v| v.get() as usize);
        let limit = self
            .limit
            .map_or(default_limit, |v| v.get() as usize)
            .min(LIST_LIMIT_MAX);
        (offset, limit)
    }
}

/// Args for `palette_probe`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaletteProbeParams {
    /// Target pid — a throwaway project, never the user's work.
    pub pid: tdmcp_core::Pid,
    /// Optional federated daemon id (omit for local / unique remote resolve).
    #[serde(default)]
    pub daemon_id: Option<String>,
    /// Which components to probe. Defaults to undescribed entries.
    #[serde(default)]
    pub select: Option<PaletteSelectorArg>,
    /// Probe blacklisted entries too. One of them may wedge TouchDesigner.
    #[serde(default)]
    pub include_ignored: bool,
    /// Structural detail level for each digest.
    #[serde(default)]
    pub detail_level: crate::tools::DetailLevel,
    /// Also render a small PNG preview per component into the palette store.
    ///
    /// The picture is rendered in the same load→destroy window as the digest
    /// (the component exists nowhere else), and is best-effort: a component
    /// that will not draw still returns a full digest.
    #[serde(default)]
    pub thumbnails: bool,
    /// Diagnostic payload size.
    #[serde(default)]
    pub diagnostic_level: tdmcp_diagnostics::DiagnosticLevel,
}

/// Default components probed in one call.
pub const PROBE_BATCH_DEFAULT: usize = 3;

/// Hard cap on components probed in one call.
///
/// One call is one bridge task, so a component that wedges TouchDesigner takes
/// its whole batch with it. Small batches keep that loss cheap and the
/// describe loop resumable.
pub const PROBE_BATCH_MAX: usize = 8;

/// Tool-layer failure carrying its diagnostic code.
#[derive(Debug)]
pub struct CodedError {
    /// Stable `tdmcp.palette.*` code.
    pub code: &'static str,
    /// Argument this is about, for the diagnostic span.
    pub field: &'static str,
    /// Human-readable detail.
    pub message: String,
}

fn store_err(e: &ProjectIoError) -> CodedError {
    CodedError {
        code: codes::PALETTE_STORE_FAILED,
        field: "action",
        message: e.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Root + store resolution
// ---------------------------------------------------------------------------

/// Palette store directory: `[palette].store_dir`, else `{data_dir}/palette`.
#[must_use]
pub fn store_dir(cfg: &ConfigFile) -> PathBuf {
    if let Some(dir) = &cfg.palette.store_dir {
        return dir.clone();
    }
    cfg.advanced
        .data_dir
        .clone()
        .unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(tdmcp_config::APP_DIR_NAME)
        })
        .join("palette")
}

/// The user's own palette folder: `[palette].user_root`, else TD's default
/// `{documents}/Derivative/Palette`. Only returned when it exists on disk —
/// same probe law as official-tool discovery.
#[must_use]
pub fn user_palette_root(cfg: &ConfigFile) -> Option<PathBuf> {
    let candidate = cfg
        .palette
        .user_root
        .clone()
        .or_else(|| dirs::document_dir().map(|d| d.join("Derivative").join("Palette")))?;
    candidate.is_dir().then_some(candidate)
}

/// Discover palette roots: every install that has one, plus the user folder.
///
/// `scan_roots` are the install-search roots (normally
/// [`resolve::default_scan_roots`]); passing them in keeps discovery
/// deterministic under test, where the host's real `/Applications` must not
/// leak into the index.
#[must_use]
pub fn discover_roots(cfg: &ConfigFile, scan_roots: &[PathBuf]) -> Vec<PaletteRoot> {
    let mut roots = Vec::new();
    // Installs come back newest-first, and one builtin palette is enough —
    // take the first that actually has one and stop looking.
    'installs: for scan_root in scan_roots {
        for exe in resolve::scan_install_exes(scan_root) {
            let info = resolve::inspect_install(&exe);
            let Some(palette) = info.palette else {
                continue;
            };
            roots.push(PaletteRoot {
                source: PaletteSource::Builtin,
                path: palette,
                install_id: info
                    .root
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .map(str::to_owned),
            });
            break 'installs;
        }
    }
    if let Some(user) = user_palette_root(cfg) {
        roots.push(PaletteRoot {
            source: PaletteSource::User,
            path: user,
            install_id: None,
        });
    }
    roots
}

// ---------------------------------------------------------------------------
// Row shaping
// ---------------------------------------------------------------------------

fn row(store: &PaletteStore, index: &PaletteIndex, id: &str, entry: &PaletteEntry) -> Value {
    let mut obj = Map::new();
    obj.insert("paletteId".into(), json!(id));
    obj.insert("name".into(), json!(entry.name));
    obj.insert("category".into(), json!(entry.category));
    obj.insert("source".into(), json!(entry.source.as_str()));
    if let Some(summary) = &entry.summary {
        obj.insert("summary".into(), json!(summary));
    }
    if !entry.tags.is_empty() {
        obj.insert("tags".into(), json!(entry.tags));
    }
    obj.insert("cardStatus".into(), json!(entry.card_status().as_str()));
    // Absolute, and only when the file is really there — a path that resolves
    // to nothing is worse than no path at all for whoever tries to open it.
    if let Some(rel) = &entry.thumb {
        let abs = store.abs_path(rel);
        if abs.is_file() {
            obj.insert("thumb".into(), json!(abs.to_string_lossy()));
        }
    }
    obj.insert("probeStatus".into(), json!(entry.probe.status.as_str()));
    if index.is_ignored(id) {
        obj.insert("ignored".into(), json!(true));
        if entry.ignored_auto {
            obj.insert("ignoredAuto".into(), json!(true));
        }
    }
    Value::Object(obj)
}

fn require_id(params: &PaletteIndexParams, action: &str) -> Result<String, CodedError> {
    params
        .palette_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| CodedError {
            code: codes::ARGS_MISSING_FIELD,
            field: "paletteId",
            message: format!("{action} requires `paletteId`"),
        })
}

fn require_entry<'a>(index: &'a PaletteIndex, id: &str) -> Result<&'a PaletteEntry, CodedError> {
    index.entries.get(id).ok_or_else(|| {
        if index.entries.is_empty() {
            CodedError {
                code: codes::PALETTE_NOT_INDEXED,
                field: "paletteId",
                message: "palette index is empty — run palette_index action=scan first".into(),
            }
        } else {
            CodedError {
                code: codes::PALETTE_UNKNOWN_ID,
                field: "paletteId",
                message: format!(
                    "no palette entry `{id}` — list candidates with palette_index action=list"
                ),
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// Execute against an explicit config + env (test seam).
pub fn run_with(
    cfg: &ConfigFile,
    scan_roots: &[PathBuf],
    params: &PaletteIndexParams,
) -> Result<Value, CodedError> {
    let store = PaletteStore::new(store_dir(cfg));
    let mut index = store.load().map_err(|e| store_err(&e))?;
    // A brand-new index inherits the shipped blacklist; an existing one keeps
    // whatever the user curated.
    let fresh = index.entries.is_empty() && index.ignore.is_empty() && index.scanned_at.is_none();
    if fresh {
        index.ignore = cfg.palette.ignore.clone();
    }

    match params.action {
        PaletteAction::Scan => action_scan(cfg, scan_roots, &store, &mut index, params),
        PaletteAction::List => action_list(&store, &index, params),
        PaletteAction::Get => action_get(&store, &index, params),
        PaletteAction::Describe => action_describe(&store, &mut index, params),
        PaletteAction::Ignore | PaletteAction::Unignore => {
            action_ignore(&store, &mut index, params)
        }
        PaletteAction::Forget => action_forget(&store, &mut index, params),
        PaletteAction::Stats => Ok(action_stats(&index)),
    }
}

/// Execute against the on-disk config and process env.
pub fn run(params: &PaletteIndexParams) -> Result<Value, CodedError> {
    let cfg = tdmcp_config::load(&tdmcp_config::default_config_path()).map_err(|e| CodedError {
        code: codes::PALETTE_STORE_FAILED,
        field: "action",
        message: format!("config load failed: {e}"),
    })?;
    run_with(
        &cfg,
        &resolve::default_scan_roots(&resolve::std_env),
        params,
    )
}

fn action_scan(
    cfg: &ConfigFile,
    scan_roots: &[PathBuf],
    store: &PaletteStore,
    index: &mut PaletteIndex,
    params: &PaletteIndexParams,
) -> Result<Value, CodedError> {
    let mut roots = discover_roots(cfg, scan_roots);
    for extra in params.roots.iter().flatten() {
        let path = PathBuf::from(extra.path.trim());
        if !path.is_dir() {
            return Err(CodedError {
                code: codes::PALETTE_NO_ROOTS,
                field: "roots",
                message: format!("palette root is not a directory: {}", path.display()),
            });
        }
        roots.retain(|r| r.path != path);
        roots.push(PaletteRoot {
            source: extra.source.into(),
            path,
            install_id: None,
        });
    }
    if roots.is_empty() {
        return Err(CodedError {
            code: codes::PALETTE_NO_ROOTS,
            field: "roots",
            message: "no palette folder found — check td_installs, or set [palette].user_root"
                .into(),
        });
    }

    // Anything still marked in-flight never reported back: that batch is what
    // wedged TD. Flag it so the agent can name and blacklist the culprit.
    let suspects = std::mem::take(&mut index.inflight);
    for id in &suspects {
        if let Some(entry) = index.entries.get_mut(id) {
            entry.probe.status = tdmcp_projectio::palette::ProbeStatus::Suspect;
        }
    }

    let report = scan_into(index, &roots);
    store.save(index).map_err(|e| store_err(&e))?;

    let mut out = json!({
        "ok": true,
        "roots": roots.iter().map(|r| json!({
            "source": r.source.as_str(),
            "path": r.path.to_string_lossy(),
            "installId": r.install_id,
        })).collect::<Vec<_>>(),
        "added": report.added,
        "updated": report.updated,
        "removed": report.removed,
        "stale": report.stale,
        "total": report.total,
        "ignored": report.ignored,
    });
    if !suspects.is_empty() {
        out["suspect"] = json!(suspects);
        out["suspectHint"] = json!(
            "these ids were mid-probe when the last run stopped — blacklist them with action=ignore before resuming"
        );
    }
    Ok(out)
}

fn action_list(
    store: &PaletteStore,
    index: &PaletteIndex,
    params: &PaletteIndexParams,
) -> Result<Value, CodedError> {
    if index.entries.is_empty() {
        return Err(CodedError {
            code: codes::PALETTE_NOT_INDEXED,
            field: "action",
            message: "palette index is empty — run palette_index action=scan first".into(),
        });
    }
    let sel = params.select.clone().unwrap_or_default();
    let matched = sel.to_selector().apply(index);
    let (offset, limit) = sel.page(LIST_LIMIT_DEFAULT);
    let total = matched.len();
    let page: Vec<Value> = matched
        .iter()
        .skip(offset)
        .take(limit)
        .filter_map(|id| index.entries.get(id).map(|e| row(store, index, id, e)))
        .collect();
    let returned = page.len();
    let mut out = json!({ "ok": true, "entries": page, "total": total });
    if offset + returned < total {
        out["truncation"] = json!({
            "reason": "page",
            "returned": returned,
            "total": total,
            "nextOffset": offset + returned,
        });
    }
    Ok(out)
}

fn action_get(
    store: &PaletteStore,
    index: &PaletteIndex,
    params: &PaletteIndexParams,
) -> Result<Value, CodedError> {
    let id = require_id(params, "get")?;
    let entry = require_entry(index, &id)?;
    let mut out = json!({
        "ok": true,
        "entry": row(store, index, &id, entry),
        "toxPath": entry.tox_path.to_string_lossy(),
    });
    if let Some(card) = &entry.card {
        match store.read_card(card) {
            Ok(body) => {
                out["card"] = json!(body);
            }
            // A card file that vanished is not worth failing the read for —
            // the entry is still useful, and re-describing restores it.
            Err(e) => {
                out["cardError"] = json!(e.to_string());
            }
        }
    }
    Ok(out)
}

fn action_describe(
    store: &PaletteStore,
    index: &mut PaletteIndex,
    params: &PaletteIndexParams,
) -> Result<Value, CodedError> {
    let id = require_id(params, "describe")?;
    require_entry(index, &id)?;
    let body = params.body.as_deref().ok_or(CodedError {
        code: codes::ARGS_MISSING_FIELD,
        field: "body",
        message: "describe requires `body` — the card Markdown".into(),
    })?;
    let summary = params.summary.as_deref().map(str::trim).ok_or(CodedError {
        code: codes::ARGS_MISSING_FIELD,
        field: "summary",
        message: "describe requires `summary` — one line, what the component does".into(),
    })?;
    if summary.is_empty() {
        return Err(CodedError {
            code: codes::ARGS_MISSING_FIELD,
            field: "summary",
            message: "summary must not be empty — it is what `list` shows".into(),
        });
    }

    let rel = store.write_card(&id, body).map_err(|e| store_err(&e))?;
    let fingerprint = index
        .entries
        .get(&id)
        .map(|e| tdmcp_projectio::palette::Fingerprint::of(&e.tox_path))
        .unwrap_or(None);
    let Some(entry) = index.entries.get_mut(&id) else {
        return Err(CodedError {
            code: codes::PALETTE_UNKNOWN_ID,
            field: "paletteId",
            message: format!("no palette entry `{id}`"),
        });
    };
    if let Some(fp) = fingerprint {
        entry.fingerprint = fp;
    }
    entry.summary = Some(summary.to_owned());
    if let Some(tags) = &params.tags {
        entry.tags = tags.clone();
    }
    entry.card = Some(rel.clone());
    entry.card_fingerprint = Some(entry.fingerprint);
    store.save(index).map_err(|e| store_err(&e))?;

    Ok(json!({
        "ok": true,
        "paletteId": id,
        "cardPath": store.dir().join(&rel).to_string_lossy(),
        "bytes": body.len(),
    }))
}

fn action_ignore(
    store: &PaletteStore,
    index: &mut PaletteIndex,
    params: &PaletteIndexParams,
) -> Result<Value, CodedError> {
    let patterns = params.patterns.clone().unwrap_or_default();
    if patterns.is_empty() {
        return Err(CodedError {
            code: codes::ARGS_MISSING_FIELD,
            field: "patterns",
            message: "ignore/unignore requires `patterns` — ids or `*` globs".into(),
        });
    }
    let adding = params.action == PaletteAction::Ignore;
    for pat in &patterns {
        let pat = pat.trim();
        if pat.is_empty() {
            continue;
        }
        if adding {
            if !index.ignore.iter().any(|p| p == pat) {
                index.ignore.push(pat.to_owned());
            }
        } else {
            index.ignore.retain(|p| p != pat);
            // An exact id also clears the per-entry flag, including an
            // auto-ignore — otherwise unignore would silently do nothing.
            if let Some(entry) = index.entries.get_mut(pat) {
                entry.ignored = false;
                entry.ignored_auto = false;
                entry.probe.fail_count = 0;
            }
        }
    }
    let affected = index
        .entries
        .keys()
        .filter(|id| index.is_ignored(id))
        .count();
    store.save(index).map_err(|e| store_err(&e))?;
    Ok(json!({ "ok": true, "ignore": index.ignore, "affected": affected }))
}

fn action_forget(
    store: &PaletteStore,
    index: &mut PaletteIndex,
    params: &PaletteIndexParams,
) -> Result<Value, CodedError> {
    let sel = params.select.clone().ok_or(CodedError {
        code: codes::ARGS_MISSING_FIELD,
        field: "select",
        message: "forget requires `select` — it will not drop the whole index by default".into(),
    })?;
    let doomed = sel.to_selector().apply(index);
    for id in &doomed {
        if let Some(entry) = index.entries.remove(id) {
            if let Some(card) = entry.card {
                // Best effort: the roster is the record, a stray card is noise.
                let _ = store.remove_card(&card);
            }
        }
    }
    store.save(index).map_err(|e| store_err(&e))?;
    Ok(json!({ "ok": true, "removed": doomed.len(), "total": index.entries.len() }))
}

fn action_stats(index: &PaletteIndex) -> Value {
    let mut described = 0usize;
    let mut stale = 0usize;
    let mut undescribed = 0usize;
    let mut failed = 0usize;
    let mut ignored = 0usize;
    let mut by_category: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    for (id, entry) in &index.entries {
        match entry.card_status() {
            CardStatus::Described => described += 1,
            CardStatus::Stale => stale += 1,
            CardStatus::Undescribed => undescribed += 1,
        }
        if matches!(
            entry.probe.status,
            tdmcp_projectio::palette::ProbeStatus::Failed
                | tdmcp_projectio::palette::ProbeStatus::Suspect
        ) {
            failed += 1;
        }
        if index.is_ignored(id) {
            ignored += 1;
        }
        let key = if entry.category.is_empty() {
            "(root)".to_owned()
        } else {
            entry.category.clone()
        };
        *by_category.entry(key).or_default() += 1;
    }
    json!({
        "ok": true,
        "total": index.entries.len(),
        "described": described,
        "stale": stale,
        "undescribed": undescribed,
        "failed": failed,
        "ignored": ignored,
        "scannedAt": index.scanned_at,
        "autoIgnoreAfter": AUTO_IGNORE_AFTER,
        "byCategory": by_category,
    })
}

// ---------------------------------------------------------------------------
// Shared with `mutate_nodes` place + `palette_probe`
// ---------------------------------------------------------------------------

/// Resolve a `paletteId` to the absolute `.tox` on disk.
///
/// Runs on the daemon **before** any bridge call, so a bad id costs nothing.
pub fn resolve_tox_path(cfg: &ConfigFile, palette_id: &str) -> Result<PathBuf, CodedError> {
    let store = PaletteStore::new(store_dir(cfg));
    let index = store.load().map_err(|e| store_err(&e))?;
    let entry = require_entry(&index, palette_id).map_err(|mut e| {
        e.field = "paletteId";
        e
    })?;
    if !entry.tox_path.is_file() {
        return Err(CodedError {
            code: codes::PALETTE_TOX_MISSING,
            field: "paletteId",
            message: format!(
                "`{palette_id}` points at {} which is gone — re-run palette_index action=scan",
                entry.tox_path.display()
            ),
        });
    }
    Ok(entry.tox_path.clone())
}

/// One component the bridge should load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeTarget {
    /// Palette id.
    pub palette_id: String,
    /// Absolute `.tox` path.
    pub tox_path: PathBuf,
}

/// How many skipped entries a probe result lists before summarizing.
///
/// A palette-wide selection can skip dozens of blacklisted components; the
/// caller needs to know *that* it happened and see examples, not read 78 rows.
pub const SKIPPED_REPORT_LIMIT: usize = 10;

/// What a probe call will do, decided entirely on the daemon.
#[derive(Debug, Clone, Default)]
pub struct ProbePlan {
    /// Components to load, in order.
    pub targets: Vec<ProbeTarget>,
    /// Entries deliberately not loaded, as `(id, reason)` — capped for report.
    pub skipped: Vec<(String, &'static str)>,
    /// Total skipped, including any beyond [`SKIPPED_REPORT_LIMIT`].
    pub skipped_total: usize,
    /// Loadable matches beyond this batch.
    pub remaining: usize,
}

impl ProbePlan {
    fn push_skip(&mut self, id: String, reason: &'static str) {
        self.skipped_total += 1;
        if self.skipped.len() < SKIPPED_REPORT_LIMIT {
            self.skipped.push((id, reason));
        }
    }
}

/// Choose this call's batch, and record it as in-flight before dispatch.
///
/// The in-flight breadcrumb is the whole point: if TouchDesigner never reports
/// back, the next `scan` can name the components that were mid-load and the
/// agent can blacklist the culprit instead of guessing.
pub fn plan_probe(cfg: &ConfigFile, params: &PaletteProbeParams) -> Result<ProbePlan, CodedError> {
    let store = PaletteStore::new(store_dir(cfg));
    let mut index = store.load().map_err(|e| store_err(&e))?;
    if index.entries.is_empty() {
        return Err(CodedError {
            code: codes::PALETTE_NOT_INDEXED,
            field: "select",
            message: "palette index is empty — run palette_index action=scan first".into(),
        });
    }

    // Default target: whatever still has no card. That is the describe loop.
    let sel_arg = params.select.clone().unwrap_or(PaletteSelectorArg {
        status: PaletteStatusArg::Undescribed,
        ..PaletteSelectorArg::default()
    });

    // Naming ids is a precise request: an id that does not exist is a mistake
    // worth reporting, not something to silently drop from the batch.
    if let Some(ids) = &sel_arg.ids {
        let unknown: Vec<&str> = ids
            .iter()
            .map(String::as_str)
            .filter(|id| !index.entries.contains_key(*id))
            .collect();
        if !unknown.is_empty() {
            return Err(CodedError {
                code: codes::PALETTE_UNKNOWN_ID,
                field: "select",
                message: format!(
                    "no palette entry for {} — list candidates with palette_index action=list",
                    unknown
                        .iter()
                        .map(|id| format!("`{id}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
    }
    // Match blacklisted entries too, then reject them per-entry below. Filtering
    // them out here instead would make a fully blacklisted selection come back
    // as a silent empty result, with nothing telling the caller why.
    let mut selector = sel_arg.to_selector();
    selector.include_ignored = true;
    let matched = selector.apply(&index);
    let (offset, limit) = sel_arg.page(PROBE_BATCH_DEFAULT);
    let limit = limit.min(PROBE_BATCH_MAX);

    let mut plan = ProbePlan::default();
    for id in matched.iter().skip(offset) {
        // Explicit ids are a deliberate act; anything else that is blacklisted
        // is reported, not loaded.
        let explicit = sel_arg
            .ids
            .as_ref()
            .is_some_and(|ids| ids.iter().any(|i| i == id));
        if index.is_ignored(id) && !params.include_ignored && !explicit {
            plan.push_skip(id.clone(), "ignored");
            continue;
        }
        let Some(entry) = index.entries.get(id) else {
            continue;
        };
        if !entry.tox_path.is_file() {
            plan.push_skip(id.clone(), "tox_missing");
            continue;
        }
        if plan.targets.len() >= limit {
            plan.remaining += 1; // Loadable, just not in this batch.
            continue;
        }
        plan.targets.push(ProbeTarget {
            palette_id: id.clone(),
            tox_path: entry.tox_path.clone(),
        });
    }

    index.inflight = plan.targets.iter().map(|t| t.palette_id.clone()).collect();
    store.save(&index).map_err(|e| store_err(&e))?;
    Ok(plan)
}

/// Drop the in-flight breadcrumb without touching the probe ledger.
///
/// For failures where the batch provably never reached TouchDesigner — no
/// connected bridge, unknown pid, queue rejection. Leaving the breadcrumb there
/// would make the next `scan` accuse innocent components of wedging TD.
pub fn clear_inflight(cfg: &ConfigFile) -> Result<(), ProjectIoError> {
    let store = PaletteStore::new(store_dir(cfg));
    let mut index = store.load()?;
    if index.inflight.is_empty() {
        return Ok(());
    }
    index.inflight.clear();
    store.save(&index)
}

/// Where each stored thumbnail landed: `paletteId` → absolute PNG path.
pub type StoredThumbs = std::collections::BTreeMap<String, String>;

/// Fold probe results back into the index and clear the in-flight breadcrumb.
///
/// `results` is the bridge's per-component array. An entry that fails twice
/// auto-ignores itself so the next bulk run does not re-hit it. Any
/// `thumbnailBase64` a row carries is decoded to `{store}/thumbs/` here, in the
/// same load→save the ledger already does, and the paths come back so the
/// caller can echo them instead of the bytes.
pub fn record_probe(cfg: &ConfigFile, results: &[Value]) -> Result<StoredThumbs, ProjectIoError> {
    use tdmcp_projectio::palette::ProbeStatus;

    let store = PaletteStore::new(store_dir(cfg));
    let mut index = store.load()?;
    let mut thumbs = StoredThumbs::new();
    index.inflight.clear();
    for row in results {
        let Some(id) = row.get("paletteId").and_then(Value::as_str) else {
            continue;
        };
        let Some(entry) = index.entries.get_mut(id) else {
            continue;
        };
        if row.get("ok").and_then(Value::as_bool) == Some(true) {
            entry.probe.status = ProbeStatus::Ok;
            entry.probe.fail_count = 0;
            entry.probe.message = None;
            entry.probe.last_ms = row.get("probeMs").and_then(Value::as_u64);
            if let Some(rel) = store_thumbnail(&store, id, row) {
                thumbs.insert(id.to_owned(), store.abs_path(&rel).to_string_lossy().into());
                entry.thumb = Some(rel);
                entry.thumb_fingerprint = Some(entry.fingerprint);
            }
        } else {
            entry.probe.status = ProbeStatus::Failed;
            entry.probe.fail_count = entry.probe.fail_count.saturating_add(1);
            entry.probe.message = row
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if entry.probe.fail_count >= AUTO_IGNORE_AFTER && !entry.ignored {
                entry.ignored = true;
                entry.ignored_auto = true;
            }
        }
    }
    store.save(&index)?;
    Ok(thumbs)
}

/// Decode and store one row's thumbnail; `None` when it has none or it is junk.
///
/// A picture is the least important thing a probe produces, so every failure
/// here — absent, unpadded base64, an unwritable store — is silence rather than
/// an error that would mask a perfectly good digest.
fn store_thumbnail(store: &PaletteStore, id: &str, row: &Value) -> Option<String> {
    use base64::Engine as _;

    let b64 = row.get("thumbnailBase64").and_then(Value::as_str)?;
    let png = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    if png.is_empty() {
        return None;
    }
    store.write_thumb(id, &png).ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// No install-scan roots: the unit tests own their palette entirely, so
    /// the host machine's real TouchDesigner never leaks in.
    const NO_INSTALLS: &[PathBuf] = &[];

    fn write_tox(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    /// Config pointing at a temp store, with a temp folder as the user palette.
    fn cfg_for(store: &Path, user_palette: &Path) -> ConfigFile {
        let mut cfg = ConfigFile::default();
        cfg.palette.store_dir = Some(store.to_path_buf());
        cfg.palette.user_root = Some(user_palette.to_path_buf());
        cfg
    }

    fn params(action: PaletteAction) -> PaletteIndexParams {
        PaletteIndexParams {
            action,
            select: None,
            palette_id: None,
            summary: None,
            tags: None,
            body: None,
            patterns: None,
            roots: None,
        }
    }

    struct Fixture {
        _store: tempfile::TempDir,
        _palette: tempfile::TempDir,
        cfg: ConfigFile,
    }

    fn fixture() -> Fixture {
        let store = tempfile::tempdir().unwrap();
        let palette = tempfile::tempdir().unwrap();
        write_tox(palette.path(), "Tools/particlesGpu.tox", b"aaaa");
        write_tox(palette.path(), "Tools/audioAnalysis.tox", b"bb");
        write_tox(palette.path(), "UI/buttons.tox", b"ccc");
        let cfg = cfg_for(store.path(), palette.path());
        Fixture {
            cfg,
            _store: store,
            _palette: palette,
        }
    }

    fn scan(cfg: &ConfigFile) -> Value {
        run_with(cfg, NO_INSTALLS, &params(PaletteAction::Scan)).unwrap()
    }

    #[test]
    fn scan_indexes_the_user_root_and_seeds_the_shipped_blacklist() {
        let fx = fixture();
        let out = scan(&fx.cfg);
        assert_eq!(out["ok"], true);
        assert_eq!(out["added"], 3);
        assert_eq!(out["total"], 3);

        let index = PaletteStore::new(store_dir(&fx.cfg)).load().unwrap();
        assert!(
            index.ignore.iter().any(|p| p == "builtin:TDAbleton/*"),
            "a fresh index inherits the config blacklist"
        );
        assert!(index.entries.contains_key("user:Tools/particlesGpu"));
    }

    #[test]
    fn list_requires_a_scan_first() {
        let fx = fixture();
        let err = run_with(&fx.cfg, NO_INSTALLS, &params(PaletteAction::List)).unwrap_err();
        assert_eq!(err.code, codes::PALETTE_NOT_INDEXED);
        assert!(err.message.contains("scan"));
    }

    #[test]
    fn list_filters_and_pages() {
        let fx = fixture();
        scan(&fx.cfg);

        let mut p = params(PaletteAction::List);
        p.select = Some(PaletteSelectorArg {
            category: Some("Tools".into()),
            ..PaletteSelectorArg::default()
        });
        let out = run_with(&fx.cfg, NO_INSTALLS, &p).unwrap();
        assert_eq!(out["total"], 2);
        assert_eq!(out["entries"].as_array().unwrap().len(), 2);

        // Paging reports how to continue rather than silently dropping rows.
        let mut p = params(PaletteAction::List);
        p.select = Some(PaletteSelectorArg {
            limit: Some(LenientU32::from(1u32)),
            ..PaletteSelectorArg::default()
        });
        let out = run_with(&fx.cfg, NO_INSTALLS, &p).unwrap();
        assert_eq!(out["entries"].as_array().unwrap().len(), 1);
        assert_eq!(out["truncation"]["total"], 3);
        assert_eq!(out["truncation"]["nextOffset"], 1);
    }

    #[test]
    fn describe_round_trips_and_clears_undescribed() {
        let fx = fixture();
        scan(&fx.cfg);

        let mut p = params(PaletteAction::Describe);
        p.palette_id = Some("user:Tools/particlesGpu".into());
        p.summary = Some("GPU particle system driven by a source TOP.".into());
        p.tags = Some(vec!["particles".into(), "gpu".into()]);
        p.body = Some("# particlesGpu\n\nbody".into());
        let out = run_with(&fx.cfg, NO_INSTALLS, &p).unwrap();
        assert_eq!(out["ok"], true);

        let mut g = params(PaletteAction::Get);
        g.palette_id = Some("user:Tools/particlesGpu".into());
        let got = run_with(&fx.cfg, NO_INSTALLS, &g).unwrap();
        assert_eq!(got["card"], "# particlesGpu\n\nbody");
        assert_eq!(got["entry"]["cardStatus"], "described");
        assert_eq!(
            got["entry"]["summary"],
            "GPU particle system driven by a source TOP."
        );

        let mut l = params(PaletteAction::List);
        l.select = Some(PaletteSelectorArg {
            status: PaletteStatusArg::Undescribed,
            ..PaletteSelectorArg::default()
        });
        let listed = run_with(&fx.cfg, NO_INSTALLS, &l).unwrap();
        assert_eq!(
            listed["total"], 2,
            "the described one drops out of the loop"
        );
    }

    #[test]
    fn describe_demands_a_summary_because_list_shows_it() {
        let fx = fixture();
        scan(&fx.cfg);
        let mut p = params(PaletteAction::Describe);
        p.palette_id = Some("user:Tools/particlesGpu".into());
        p.body = Some("body".into());
        let err = run_with(&fx.cfg, NO_INSTALLS, &p).unwrap_err();
        assert_eq!(err.field, "summary");

        p.summary = Some("   ".into());
        let err = run_with(&fx.cfg, NO_INSTALLS, &p).unwrap_err();
        assert_eq!(err.field, "summary");
    }

    #[test]
    fn describe_rejects_an_unknown_id() {
        let fx = fixture();
        scan(&fx.cfg);
        let mut p = params(PaletteAction::Describe);
        p.palette_id = Some("user:Nope/missing".into());
        p.summary = Some("x".into());
        p.body = Some("y".into());
        let err = run_with(&fx.cfg, NO_INSTALLS, &p).unwrap_err();
        assert_eq!(err.code, codes::PALETTE_UNKNOWN_ID);
    }

    #[test]
    fn ignore_and_unignore_move_entries_in_and_out_of_the_blacklist() {
        let fx = fixture();
        scan(&fx.cfg);

        let mut p = params(PaletteAction::Ignore);
        p.patterns = Some(vec!["user:UI/*".into()]);
        let out = run_with(&fx.cfg, NO_INSTALLS, &p).unwrap();
        assert_eq!(out["affected"], 1);

        let mut l = params(PaletteAction::List);
        let listed = run_with(&fx.cfg, NO_INSTALLS, &l).unwrap();
        assert_eq!(listed["total"], 2, "blacklisted entries drop out of list");

        l.select = Some(PaletteSelectorArg {
            include_ignored: true,
            ..PaletteSelectorArg::default()
        });
        let listed = run_with(&fx.cfg, NO_INSTALLS, &l).unwrap();
        assert_eq!(listed["total"], 3);

        let mut u = params(PaletteAction::Unignore);
        u.patterns = Some(vec!["user:UI/*".into()]);
        let out = run_with(&fx.cfg, NO_INSTALLS, &u).unwrap();
        assert_eq!(out["affected"], 0);
    }

    #[test]
    fn unignore_clears_a_per_entry_auto_ignore() {
        let fx = fixture();
        scan(&fx.cfg);
        let store = PaletteStore::new(store_dir(&fx.cfg));
        let mut index = store.load().unwrap();
        let entry = index.entries.get_mut("user:Tools/particlesGpu").unwrap();
        entry.ignored = true;
        entry.ignored_auto = true;
        entry.probe.fail_count = AUTO_IGNORE_AFTER;
        store.save(&index).unwrap();

        let mut u = params(PaletteAction::Unignore);
        u.patterns = Some(vec!["user:Tools/particlesGpu".into()]);
        run_with(&fx.cfg, NO_INSTALLS, &u).unwrap();

        let index = store.load().unwrap();
        let entry = &index.entries["user:Tools/particlesGpu"];
        assert!(!entry.ignored && !entry.ignored_auto);
        assert_eq!(entry.probe.fail_count, 0);
    }

    #[test]
    fn forget_refuses_to_wipe_the_index_without_a_selector() {
        let fx = fixture();
        scan(&fx.cfg);
        let err = run_with(&fx.cfg, NO_INSTALLS, &params(PaletteAction::Forget)).unwrap_err();
        assert_eq!(err.field, "select");

        let mut p = params(PaletteAction::Forget);
        p.select = Some(PaletteSelectorArg {
            category: Some("UI".into()),
            ..PaletteSelectorArg::default()
        });
        let out = run_with(&fx.cfg, NO_INSTALLS, &p).unwrap();
        assert_eq!(out["removed"], 1);
        assert_eq!(out["total"], 2);
    }

    #[test]
    fn scan_flags_an_abandoned_probe_batch_as_suspect() {
        let fx = fixture();
        scan(&fx.cfg);
        let store = PaletteStore::new(store_dir(&fx.cfg));
        let mut index = store.load().unwrap();
        index.inflight = vec!["user:Tools/particlesGpu".into()];
        store.save(&index).unwrap();

        let out = scan(&fx.cfg);
        assert_eq!(out["suspect"][0], "user:Tools/particlesGpu");
        assert!(out["suspectHint"].as_str().unwrap().contains("ignore"));

        let index = store.load().unwrap();
        assert!(index.inflight.is_empty(), "the breadcrumb is consumed once");
        assert_eq!(
            index.entries["user:Tools/particlesGpu"]
                .probe
                .status
                .as_str(),
            "suspect"
        );

        let mut l = params(PaletteAction::List);
        l.select = Some(PaletteSelectorArg {
            status: PaletteStatusArg::Failed,
            ..PaletteSelectorArg::default()
        });
        let listed = run_with(&fx.cfg, NO_INSTALLS, &l).unwrap();
        assert_eq!(listed["total"], 1);
    }

    #[test]
    fn stats_counts_the_slice_the_describe_loop_watches() {
        let fx = fixture();
        scan(&fx.cfg);
        let out = run_with(&fx.cfg, NO_INSTALLS, &params(PaletteAction::Stats)).unwrap();
        assert_eq!(out["total"], 3);
        assert_eq!(out["undescribed"], 3);
        assert_eq!(out["byCategory"]["Tools"], 2);
    }

    #[test]
    fn scan_rejects_a_root_that_is_not_a_directory() {
        let fx = fixture();
        let mut p = params(PaletteAction::Scan);
        p.roots = Some(vec![PaletteRootArg {
            source: PaletteSourceArg::User,
            path: "/definitely/not/here".into(),
        }]);
        let err = run_with(&fx.cfg, NO_INSTALLS, &p).unwrap_err();
        assert_eq!(err.code, codes::PALETTE_NO_ROOTS);
    }

    fn probe_params(select: Option<PaletteSelectorArg>) -> PaletteProbeParams {
        PaletteProbeParams {
            pid: tdmcp_core::Pid::new(1234),
            daemon_id: None,
            select,
            include_ignored: false,
            detail_level: crate::tools::DetailLevel::default(),
            thumbnails: false,
            diagnostic_level: tdmcp_diagnostics::DiagnosticLevel::default(),
        }
    }

    #[test]
    fn probe_defaults_to_the_undescribed_slice_in_small_batches() {
        let fx = fixture();
        scan(&fx.cfg);
        let plan = plan_probe(&fx.cfg, &probe_params(None)).unwrap();
        assert_eq!(plan.targets.len(), PROBE_BATCH_DEFAULT);
        assert_eq!(plan.remaining, 0, "3 entries, batch of 3");
        assert!(plan.targets.iter().all(|t| t.tox_path.is_file()));
    }

    #[test]
    fn probe_batch_is_capped_however_large_the_limit_asked_for() {
        let fx = fixture();
        scan(&fx.cfg);
        let plan = plan_probe(
            &fx.cfg,
            &probe_params(Some(PaletteSelectorArg {
                limit: Some(LenientU32::from(500u32)),
                ..PaletteSelectorArg::default()
            })),
        )
        .unwrap();
        assert!(plan.targets.len() <= PROBE_BATCH_MAX);
    }

    #[test]
    fn an_all_blacklisted_selection_reports_why_nothing_ran() {
        // Silently returning an empty batch would read as "the palette is
        // empty" instead of "everything you asked for is blacklisted".
        let fx = fixture();
        scan(&fx.cfg);
        let mut ig = params(PaletteAction::Ignore);
        ig.patterns = Some(vec!["user:Tools/*".into()]);
        run_with(&fx.cfg, NO_INSTALLS, &ig).unwrap();

        let plan = plan_probe(
            &fx.cfg,
            &probe_params(Some(PaletteSelectorArg {
                category: Some("Tools".into()),
                ..PaletteSelectorArg::default()
            })),
        )
        .unwrap();
        assert!(plan.targets.is_empty());
        assert_eq!(plan.skipped_total, 2);
        assert!(plan.skipped.iter().all(|(_, why)| *why == "ignored"));
    }

    #[test]
    fn the_skipped_report_is_capped_but_the_count_is_not() {
        let fx = fixture();
        scan(&fx.cfg);
        let mut ig = params(PaletteAction::Ignore);
        ig.patterns = Some(vec!["user:*".into()]);
        run_with(&fx.cfg, NO_INSTALLS, &ig).unwrap();
        let plan = plan_probe(&fx.cfg, &probe_params(None)).unwrap();
        assert_eq!(plan.skipped_total, 3);
        assert!(plan.skipped.len() <= SKIPPED_REPORT_LIMIT);
    }

    #[test]
    fn blacklisted_entries_do_not_eat_the_batch_budget() {
        let fx = fixture();
        scan(&fx.cfg);
        let mut ig = params(PaletteAction::Ignore);
        ig.patterns = Some(vec!["user:Tools/audioAnalysis".into()]);
        run_with(&fx.cfg, NO_INSTALLS, &ig).unwrap();

        let plan = plan_probe(
            &fx.cfg,
            &probe_params(Some(PaletteSelectorArg {
                limit: Some(LenientU32::from(1u32)),
                ..PaletteSelectorArg::default()
            })),
        )
        .unwrap();
        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.skipped_total, 1);
        // One loadable entry is left over — the skipped one is not "remaining".
        assert_eq!(plan.remaining, 1);
    }

    #[test]
    fn probe_skips_blacklisted_entries_and_says_why() {
        let fx = fixture();
        scan(&fx.cfg);
        let mut ig = params(PaletteAction::Ignore);
        ig.patterns = Some(vec!["user:UI/*".into()]);
        run_with(&fx.cfg, NO_INSTALLS, &ig).unwrap();

        let plan = plan_probe(&fx.cfg, &probe_params(None)).unwrap();
        assert_eq!(plan.targets.len(), 2);
        assert!(!plan
            .targets
            .iter()
            .any(|t| t.palette_id.starts_with("user:UI/")));

        // Naming it explicitly is a deliberate act and goes through.
        let plan = plan_probe(
            &fx.cfg,
            &probe_params(Some(PaletteSelectorArg {
                ids: Some(vec!["user:UI/buttons".into()]),
                ..PaletteSelectorArg::default()
            })),
        )
        .unwrap();
        assert_eq!(plan.targets.len(), 1);
    }

    #[test]
    fn probe_records_the_batch_in_flight_before_dispatch() {
        let fx = fixture();
        scan(&fx.cfg);
        let plan = plan_probe(&fx.cfg, &probe_params(None)).unwrap();
        let index = PaletteStore::new(store_dir(&fx.cfg)).load().unwrap();
        assert_eq!(index.inflight.len(), plan.targets.len());
        assert!(index
            .inflight
            .contains(&"user:Tools/particlesGpu".to_string()));
    }

    #[test]
    fn clearing_the_breadcrumb_leaves_the_probe_ledger_alone() {
        // A dispatch that never reached TD must not accuse its components of
        // wedging anything — but it must not reset their history either.
        let fx = fixture();
        scan(&fx.cfg);
        record_probe(
            &fx.cfg,
            &[json!({"ok": false, "paletteId": "user:UI/buttons"})],
        )
        .unwrap();
        plan_probe(&fx.cfg, &probe_params(None)).unwrap();

        let store = PaletteStore::new(store_dir(&fx.cfg));
        assert!(!store.load().unwrap().inflight.is_empty());

        clear_inflight(&fx.cfg).unwrap();
        let index = store.load().unwrap();
        assert!(index.inflight.is_empty());
        assert_eq!(index.entries["user:UI/buttons"].probe.fail_count, 1);

        // Idempotent — a second clear on an empty breadcrumb is a no-op.
        clear_inflight(&fx.cfg).unwrap();
    }

    #[test]
    fn probe_reports_a_vanished_tox_instead_of_sending_it_to_td() {
        let fx = fixture();
        scan(&fx.cfg);
        let index = PaletteStore::new(store_dir(&fx.cfg)).load().unwrap();
        fs::remove_file(&index.entries["user:Tools/particlesGpu"].tox_path).unwrap();

        let plan = plan_probe(&fx.cfg, &probe_params(None)).unwrap();
        assert!(plan
            .skipped
            .iter()
            .any(|(id, why)| id == "user:Tools/particlesGpu" && *why == "tox_missing"));
        assert!(!plan
            .targets
            .iter()
            .any(|t| t.palette_id == "user:Tools/particlesGpu"));
    }

    #[test]
    fn probe_names_the_ids_it_could_not_find() {
        // Silently returning an empty batch would look like "already described".
        let fx = fixture();
        scan(&fx.cfg);
        let err = plan_probe(
            &fx.cfg,
            &probe_params(Some(PaletteSelectorArg {
                ids: Some(vec![
                    "user:Tools/particlesGpu".into(),
                    "builtin:Nope/ghost".into(),
                ]),
                ..PaletteSelectorArg::default()
            })),
        )
        .unwrap_err();
        assert_eq!(err.code, codes::PALETTE_UNKNOWN_ID);
        assert!(err.message.contains("builtin:Nope/ghost"));
        assert!(!err.message.contains("particlesGpu"), "only the bad ids");
    }

    #[test]
    fn probe_requires_a_scan_first() {
        let fx = fixture();
        let err = plan_probe(&fx.cfg, &probe_params(None)).unwrap_err();
        assert_eq!(err.code, codes::PALETTE_NOT_INDEXED);
    }

    #[test]
    fn recording_results_clears_the_breadcrumb_and_auto_ignores_repeat_failures() {
        let fx = fixture();
        scan(&fx.cfg);
        plan_probe(&fx.cfg, &probe_params(None)).unwrap();

        let bad = json!({
            "ok": false,
            "paletteId": "user:UI/buttons",
            "message": "socket timeout",
        });
        let good = json!({
            "ok": true,
            "paletteId": "user:Tools/particlesGpu",
            "probeMs": 812,
        });
        record_probe(&fx.cfg, &[bad.clone(), good]).unwrap();

        let store = PaletteStore::new(store_dir(&fx.cfg));
        let index = store.load().unwrap();
        assert!(
            index.inflight.is_empty(),
            "the breadcrumb is cleared on report"
        );
        assert_eq!(
            index.entries["user:Tools/particlesGpu"].probe.last_ms,
            Some(812)
        );
        let failed = &index.entries["user:UI/buttons"];
        assert_eq!(failed.probe.fail_count, 1);
        assert!(!failed.ignored, "one failure is not yet a blacklist");

        // Second strike: it blacklists itself so the next bulk run skips it.
        record_probe(&fx.cfg, &[bad]).unwrap();
        let index = store.load().unwrap();
        let failed = &index.entries["user:UI/buttons"];
        assert_eq!(failed.probe.fail_count, AUTO_IGNORE_AFTER);
        assert!(failed.ignored && failed.ignored_auto);
        assert_eq!(failed.probe.message.as_deref(), Some("socket timeout"));

        let plan = plan_probe(&fx.cfg, &probe_params(None)).unwrap();
        assert!(!plan
            .targets
            .iter()
            .any(|t| t.palette_id == "user:UI/buttons"));
    }

    #[test]
    fn a_probed_thumbnail_lands_in_the_store_and_shows_up_in_list() {
        let fx = fixture();
        scan(&fx.cfg);
        let id = "user:Tools/particlesGpu";
        // "PNG" is enough: nothing here decodes the image, only moves bytes.
        let png_b64 = "iVBORw0KGgo=";

        let thumbs = record_probe(
            &fx.cfg,
            &[json!({"ok": true, "paletteId": id, "thumbnailBase64": png_b64})],
        )
        .unwrap();
        let abs = thumbs.get(id).expect("the path comes back for the caller");
        assert!(std::path::Path::new(abs).is_file());

        let store = PaletteStore::new(store_dir(&fx.cfg));
        let entry = &store.load().unwrap().entries[id];
        assert!(entry.thumb.as_deref().unwrap().starts_with("thumbs/"));
        assert_eq!(
            entry.thumb_fingerprint,
            Some(entry.fingerprint),
            "a fresh render is current, not stale"
        );

        // The GUI reads it off `list` like any other field.
        let out = run_with(&fx.cfg, NO_INSTALLS, &params(PaletteAction::List)).unwrap();
        let row = out["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["paletteId"] == id)
            .unwrap();
        assert_eq!(row["thumb"], json!(abs));
    }

    #[test]
    fn a_thumbnail_path_is_withheld_once_the_file_is_gone() {
        // A path that resolves to nothing is worse than no path at all.
        let fx = fixture();
        scan(&fx.cfg);
        let id = "user:Tools/particlesGpu";
        record_probe(
            &fx.cfg,
            &[json!({"ok": true, "paletteId": id, "thumbnailBase64": "iVBORw0KGgo="})],
        )
        .unwrap();

        let store = PaletteStore::new(store_dir(&fx.cfg));
        let rel = store.load().unwrap().entries[id].thumb.clone().unwrap();
        store.remove_thumb(&rel).unwrap();

        let out = run_with(&fx.cfg, NO_INSTALLS, &params(PaletteAction::List)).unwrap();
        let row = out["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["paletteId"] == id)
            .unwrap();
        assert!(row.get("thumb").is_none());
    }

    #[test]
    fn junk_thumbnail_bytes_never_break_a_good_digest() {
        let fx = fixture();
        scan(&fx.cfg);
        let id = "user:Tools/particlesGpu";
        for junk in ["", "!!!not base64!!!"] {
            let thumbs = record_probe(
                &fx.cfg,
                &[json!({"ok": true, "paletteId": id, "thumbnailBase64": junk, "probeMs": 5})],
            )
            .unwrap();
            assert!(thumbs.is_empty(), "junk stores nothing");
            let index = PaletteStore::new(store_dir(&fx.cfg)).load().unwrap();
            // The digest half of the row still landed.
            assert_eq!(index.entries[id].probe.last_ms, Some(5));
            assert!(index.entries[id].thumb.is_none());
        }
    }

    #[test]
    fn a_success_after_a_failure_resets_the_strike_count() {
        let fx = fixture();
        scan(&fx.cfg);
        let id = "user:UI/buttons";
        record_probe(&fx.cfg, &[json!({"ok": false, "paletteId": id})]).unwrap();
        record_probe(&fx.cfg, &[json!({"ok": true, "paletteId": id})]).unwrap();
        let index = PaletteStore::new(store_dir(&fx.cfg)).load().unwrap();
        assert_eq!(index.entries[id].probe.fail_count, 0);
        assert_eq!(index.entries[id].probe.status.as_str(), "ok");
    }

    #[test]
    fn resolve_tox_path_fails_before_any_bridge_call() {
        let fx = fixture();

        // Nothing scanned yet.
        let err = resolve_tox_path(&fx.cfg, "user:Tools/particlesGpu").unwrap_err();
        assert_eq!(err.code, codes::PALETTE_NOT_INDEXED);

        scan(&fx.cfg);
        let path = resolve_tox_path(&fx.cfg, "user:Tools/particlesGpu").unwrap();
        assert!(path.is_file());

        let err = resolve_tox_path(&fx.cfg, "user:Tools/nope").unwrap_err();
        assert_eq!(err.code, codes::PALETTE_UNKNOWN_ID);

        fs::remove_file(&path).unwrap();
        let err = resolve_tox_path(&fx.cfg, "user:Tools/particlesGpu").unwrap_err();
        assert_eq!(err.code, codes::PALETTE_TOX_MISSING);
    }
}
