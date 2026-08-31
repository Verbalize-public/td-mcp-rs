//! Palette component index + card store (`{store_dir}/index.json` + `cards/`).
//!
//! TouchDesigner's Palette is a folder tree of `.tox` components — Derivative's
//! own under the install root, plus the user's own palette folder. This module
//! owns the offline half of palette awareness:
//!
//! - **discovery** — walk palette roots, derive stable ids, fingerprint files
//! - **persistence** — the JSON roster plus one Markdown card per described
//!   component
//! - **selection** — the id-glob / category / status filters every bulk action
//!   uses, so "analyse only part of the palette" lives in exactly one place
//!
//! Cards are *authored by the agent* from live probe evidence, never generated
//! here — this module only stores and serves them. Crate-boundary law holds: no
//! config-crate dependency, callers pass resolved paths in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ProjectIoError;

/// Index schema version; bumped when the on-disk shape changes incompatibly.
pub const INDEX_VERSION: u32 = 1;

/// Default page size for `list`.
pub const LIST_LIMIT_DEFAULT: usize = 50;

/// Hard cap on entries returned by one `list` call.
pub const LIST_LIMIT_MAX: usize = 500;

/// Consecutive probe failures after which an entry auto-ignores itself.
pub const AUTO_IGNORE_AFTER: u32 = 2;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// Which palette root an entry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaletteSource {
    /// Shipped with the TouchDesigner install.
    Builtin,
    /// The user's own palette folder.
    User,
}

impl PaletteSource {
    /// Wire string used as the id prefix.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::User => "user",
        }
    }
}

/// One scanned palette root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteRoot {
    /// Which half of the palette this root provides.
    pub source: PaletteSource,
    /// Absolute folder path.
    pub path: PathBuf,
    /// Owning install id, when the root came from a TD install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_id: Option<String>,
}

/// Cheap change detector for a `.tox` on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    /// File size in bytes.
    pub bytes: u64,
    /// Modification time, milliseconds since the Unix epoch.
    pub mtime_ms: i64,
}

impl Fingerprint {
    /// Read size + mtime for `path`. Missing/unreadable metadata → `None`.
    #[must_use]
    pub fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .unwrap_or(0);
        Some(Self {
            bytes: meta.len(),
            mtime_ms,
        })
    }
}

/// Outcome of the most recent probe attempt on an entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProbeStatus {
    /// Never probed.
    #[default]
    Unprobed,
    /// Probe returned a digest.
    Ok,
    /// Probe returned an error row for this component.
    Failed,
    /// Probe was dispatched but no result came back — TD likely wedged on it.
    Suspect,
    /// Probe skipped it (ignored).
    Skipped,
}

impl ProbeStatus {
    /// Wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unprobed => "unprobed",
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Suspect => "suspect",
            Self::Skipped => "skipped",
        }
    }
}

/// Probe ledger for one entry — what makes a bulk run survivable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeState {
    /// Latest attempt outcome.
    #[serde(default)]
    pub status: ProbeStatus,
    /// Consecutive failures; at [`AUTO_IGNORE_AFTER`] the entry auto-ignores.
    #[serde(default)]
    pub fail_count: u32,
    /// Wall time of the last successful probe, milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ms: Option<u64>,
    /// Last failure detail, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// One palette component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteEntry {
    /// Leaf name without the `.tox` extension.
    pub name: String,
    /// Folder path under the root (`Tools`, `UI/Basic Widgets`); empty at root level.
    pub category: String,
    /// Which root it came from.
    pub source: PaletteSource,
    /// Absolute path to the `.tox`.
    pub tox_path: PathBuf,
    /// Size + mtime as of the last scan.
    pub fingerprint: Fingerprint,
    /// One-line agent-written summary; what makes `list` cheap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Agent-written retrieval tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Card file path relative to the store dir, when described.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<String>,
    /// Fingerprint the card was written against; drift ⇒ the card is stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_fingerprint: Option<Fingerprint>,
    /// Probe ledger.
    #[serde(default)]
    pub probe: ProbeState,
    /// Explicitly ignored (never probed unless forced).
    #[serde(default)]
    pub ignored: bool,
    /// The ignore was set by the auto-ignore rule, not by a human.
    #[serde(default)]
    pub ignored_auto: bool,
}

impl PaletteEntry {
    /// Card state relative to the `.tox` on disk.
    #[must_use]
    pub fn card_status(&self) -> CardStatus {
        match (&self.card, self.card_fingerprint) {
            (Some(_), Some(fp)) if fp == self.fingerprint => CardStatus::Described,
            (Some(_), _) => CardStatus::Stale,
            (None, _) => CardStatus::Undescribed,
        }
    }
}

/// Whether an entry has a usable card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardStatus {
    /// No card written yet.
    Undescribed,
    /// Card matches the current `.tox`.
    Described,
    /// Card exists but the `.tox` changed under it.
    Stale,
}

impl CardStatus {
    /// Wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Undescribed => "undescribed",
            Self::Described => "described",
            Self::Stale => "stale",
        }
    }
}

/// The persisted roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteIndex {
    /// Schema version.
    pub version: u32,
    /// RFC3339 timestamp of the last scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanned_at: Option<String>,
    /// Roots covered by the last scan.
    #[serde(default)]
    pub roots: Vec<PaletteRoot>,
    /// Ignore globs matched against ids.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Ids dispatched to a probe that has not reported back.
    #[serde(default)]
    pub inflight: Vec<String>,
    /// id → entry.
    #[serde(default)]
    pub entries: BTreeMap<String, PaletteEntry>,
}

impl Default for PaletteIndex {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            scanned_at: None,
            roots: Vec::new(),
            ignore: Vec::new(),
            inflight: Vec::new(),
            entries: BTreeMap::new(),
        }
    }
}

impl PaletteIndex {
    /// Whether `id` is ignored — by entry flag or by any ignore glob.
    #[must_use]
    pub fn is_ignored(&self, id: &str) -> bool {
        if self.entries.get(id).is_some_and(|e| e.ignored) {
            return true;
        }
        self.ignore.iter().any(|pat| glob_match(pat, id))
    }
}

// ---------------------------------------------------------------------------
// Id derivation
// ---------------------------------------------------------------------------

/// Build a palette id from a source and a root-relative `.tox` path.
///
/// `builtin` + `Tools/particlesGpu.tox` → `builtin:Tools/particlesGpu`.
#[must_use]
pub fn palette_id(source: PaletteSource, rel: &Path) -> String {
    let stem: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let mut joined = stem.join("/");
    if let Some(trimmed) = joined.strip_suffix(".tox") {
        joined = trimmed.to_string();
    }
    format!("{}:{}", source.as_str(), joined)
}

/// Split an id into `(category, name)`. `builtin:UI/Basic Widgets/btn` →
/// `("UI/Basic Widgets", "btn")`.
#[must_use]
pub fn split_id(id: &str) -> (String, String) {
    let body = id.split_once(':').map_or(id, |(_, rest)| rest);
    match body.rsplit_once('/') {
        Some((cat, name)) => (cat.to_string(), name.to_string()),
        None => (String::new(), body.to_string()),
    }
}

/// Filesystem-safe card filename for an id, collision-proofed with a short hash.
#[must_use]
pub fn card_slug(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // FNV-1a 32-bit — same family as the bootstrap.tox source stamp.
    let mut hash: u32 = 0x811c_9dc5;
    for b in id.as_bytes() {
        hash ^= u32::from(*b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    let trimmed: String = sanitized.chars().take(80).collect();
    format!("{trimmed}-{hash:08x}.md")
}

// ---------------------------------------------------------------------------
// Glob matching (ids only; `*` and `?`)
// ---------------------------------------------------------------------------

/// Match `text` against a `*` / `?` glob. `*` crosses `/` — ids are flat enough
/// that a path-aware glob would only surprise.
#[must_use]
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    // Iterative backtracking: linear in the common case, no recursion depth risk.
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ti;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

/// Status filter for a selector.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SelectStatus {
    /// No status filter.
    #[default]
    All,
    /// No card yet.
    Undescribed,
    /// Card present and current.
    Described,
    /// Card present but the `.tox` changed.
    Stale,
    /// Last probe failed.
    Failed,
    /// Ignored (by flag or glob).
    Ignored,
}

/// Which entries a bulk action applies to. One type for `list`, `forget`, and
/// `palette_probe`, so partial-palette selection is defined once.
#[derive(Debug, Clone, Default)]
pub struct Selector {
    /// Exact ids.
    pub ids: Vec<String>,
    /// Category prefix (matches `Tools` and `Tools/Sub`).
    pub category: Option<String>,
    /// Restrict to one root.
    pub source: Option<PaletteSource>,
    /// Glob over the id.
    pub pattern: Option<String>,
    /// Card / probe status.
    pub status: SelectStatus,
    /// Include entries that are ignored (default: exclude, except `status=ignored`).
    pub include_ignored: bool,
}

impl Selector {
    /// Ids matching this selector, in index order.
    #[must_use]
    pub fn apply(&self, index: &PaletteIndex) -> Vec<String> {
        index
            .entries
            .iter()
            .filter(|(id, entry)| self.matches(index, id, entry))
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn matches(&self, index: &PaletteIndex, id: &str, entry: &PaletteEntry) -> bool {
        if !self.ids.is_empty() && !self.ids.iter().any(|want| want == id) {
            return false;
        }
        if let Some(cat) = &self.category {
            let hit = entry.category == *cat
                || entry
                    .category
                    .strip_prefix(cat)
                    .is_some_and(|rest| rest.starts_with('/'));
            if !hit {
                return false;
            }
        }
        if let Some(src) = self.source {
            if entry.source != src {
                return false;
            }
        }
        if let Some(pat) = &self.pattern {
            if !glob_match(pat, id) {
                return false;
            }
        }
        let ignored = index.is_ignored(id);
        match self.status {
            SelectStatus::All => {}
            SelectStatus::Undescribed => {
                if entry.card_status() != CardStatus::Undescribed {
                    return false;
                }
            }
            SelectStatus::Described => {
                if entry.card_status() != CardStatus::Described {
                    return false;
                }
            }
            SelectStatus::Stale => {
                if entry.card_status() != CardStatus::Stale {
                    return false;
                }
            }
            SelectStatus::Failed => {
                if !matches!(
                    entry.probe.status,
                    ProbeStatus::Failed | ProbeStatus::Suspect
                ) {
                    return false;
                }
            }
            SelectStatus::Ignored => return ignored,
        }
        // Explicit ids are a deliberate act; they bypass the ignore filter.
        self.include_ignored || !self.ids.is_empty() || !ignored
    }
}

// ---------------------------------------------------------------------------
// Scan
// ---------------------------------------------------------------------------

/// What one scan changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// Ids newly seen.
    pub added: usize,
    /// Ids whose `.tox` fingerprint changed.
    pub updated: usize,
    /// Ids whose file disappeared from a scanned root.
    pub removed: usize,
    /// Entries whose card no longer matches its `.tox`.
    pub stale: usize,
    /// Entries in the index after the scan.
    pub total: usize,
    /// Entries currently ignored.
    pub ignored: usize,
}

/// Every `.tox` under `root`, recursively, as `(id, relative path)`.
#[must_use]
pub fn walk_palette_root(root: &Path, source: PaletteSource) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'));
            if hidden {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "tox") {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push((palette_id(source, rel), path.clone()));
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Rescan `roots` into `index`, reconciling adds / updates / removals.
///
/// Only sources actually present in `roots` are pruned — scanning with just a
/// user root never drops the builtin roster.
pub fn scan_into(index: &mut PaletteIndex, roots: &[PaletteRoot]) -> ScanReport {
    let mut report = ScanReport::default();
    let mut seen: Vec<String> = Vec::new();

    for root in roots {
        for (id, tox_path) in walk_palette_root(&root.path, root.source) {
            let Some(fingerprint) = Fingerprint::of(&tox_path) else {
                continue;
            };
            seen.push(id.clone());
            let (category, name) = split_id(&id);
            match index.entries.get_mut(&id) {
                Some(existing) => {
                    existing.tox_path = tox_path;
                    existing.category = category;
                    existing.name = name;
                    if existing.fingerprint != fingerprint {
                        existing.fingerprint = fingerprint;
                        report.updated += 1;
                    }
                }
                None => {
                    index.entries.insert(
                        id,
                        PaletteEntry {
                            name,
                            category,
                            source: root.source,
                            tox_path,
                            fingerprint,
                            summary: None,
                            tags: Vec::new(),
                            card: None,
                            card_fingerprint: None,
                            probe: ProbeState::default(),
                            ignored: false,
                            ignored_auto: false,
                        },
                    );
                    report.added += 1;
                }
            }
        }
    }

    let scanned_sources: Vec<PaletteSource> = roots.iter().map(|r| r.source).collect();
    let doomed: Vec<String> = index
        .entries
        .iter()
        .filter(|(id, entry)| {
            scanned_sources.contains(&entry.source) && !seen.iter().any(|s| s == *id)
        })
        .map(|(id, _)| id.clone())
        .collect();
    report.removed = doomed.len();
    for id in doomed {
        index.entries.remove(&id);
    }

    index.roots = roots.to_vec();
    index.scanned_at = Some(chrono::Utc::now().to_rfc3339());
    report.total = index.entries.len();
    report.stale = index
        .entries
        .values()
        .filter(|e| e.card_status() == CardStatus::Stale)
        .count();
    report.ignored = index
        .entries
        .keys()
        .filter(|id| index.is_ignored(id))
        .count();
    report
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// On-disk palette store: `index.json` plus a `cards/` folder.
#[derive(Debug, Clone)]
pub struct PaletteStore {
    dir: PathBuf,
}

impl PaletteStore {
    /// Bind a store to `dir` (created lazily on first write).
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Store root.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `{dir}/index.json`.
    #[must_use]
    pub fn index_path(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    /// `{dir}/cards`.
    #[must_use]
    pub fn cards_dir(&self) -> PathBuf {
        self.dir.join("cards")
    }

    /// Load the index. A missing file is an empty index, not an error; a
    /// corrupt one is [`ProjectIoError::PaletteStore`] so the caller can tell
    /// the user to re-scan rather than silently discarding their cards.
    pub fn load(&self) -> Result<PaletteIndex, ProjectIoError> {
        let path = self.index_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(PaletteIndex::default())
            }
            Err(source) => return Err(ProjectIoError::Fs { path, source }),
        };
        let index: PaletteIndex =
            serde_json::from_str(&text).map_err(|e| ProjectIoError::PaletteStore {
                path: path.clone(),
                reason: format!("index.json is not readable ({e}); re-run scan to rebuild"),
            })?;
        if index.version != INDEX_VERSION {
            return Err(ProjectIoError::PaletteStore {
                path,
                reason: format!(
                    "index.json is version {} but this build expects {INDEX_VERSION}; re-run scan to rebuild",
                    index.version
                ),
            });
        }
        Ok(index)
    }

    /// Write the index (pretty JSON, parent dirs created).
    pub fn save(&self, index: &PaletteIndex) -> Result<(), ProjectIoError> {
        let path = self.index_path();
        std::fs::create_dir_all(&self.dir).map_err(|source| ProjectIoError::Fs {
            path: self.dir.clone(),
            source,
        })?;
        let body =
            serde_json::to_string_pretty(index).map_err(|e| ProjectIoError::PaletteStore {
                path: path.clone(),
                reason: format!("index serialization failed: {e}"),
            })?;
        std::fs::write(&path, body).map_err(|source| ProjectIoError::Fs { path, source })
    }

    /// Read a card body by its store-relative path.
    pub fn read_card(&self, rel: &str) -> Result<String, ProjectIoError> {
        let path = self.dir.join(rel);
        std::fs::read_to_string(&path).map_err(|source| ProjectIoError::Fs { path, source })
    }

    /// Write a card body for `id`; returns the store-relative path.
    pub fn write_card(&self, id: &str, body: &str) -> Result<String, ProjectIoError> {
        let cards = self.cards_dir();
        std::fs::create_dir_all(&cards).map_err(|source| ProjectIoError::Fs {
            path: cards.clone(),
            source,
        })?;
        let rel = format!("cards/{}", card_slug(id));
        let path = self.dir.join(&rel);
        std::fs::write(&path, body).map_err(|source| ProjectIoError::Fs { path, source })?;
        Ok(rel)
    }

    /// Delete a card by store-relative path. A missing file is not an error.
    pub fn remove_card(&self, rel: &str) -> Result<(), ProjectIoError> {
        let path = self.dir.join(rel);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(ProjectIoError::Fs { path, source }),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use std::fs;

    fn tox(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn fake_builtin(root: &Path) {
        tox(root, "Tools/particlesGpu.tox", b"aaaa");
        tox(root, "Tools/audioAnalysis.tox", b"bb");
        tox(root, "UI/Basic Widgets/buttons.tox", b"ccc");
        tox(root, "template.tox", b"d");
        tox(root, ".hidden/skipme.tox", b"e");
        tox(root, "Tools/notes.txt", b"not a component");
    }

    fn roots(dir: &Path) -> Vec<PaletteRoot> {
        vec![PaletteRoot {
            source: PaletteSource::Builtin,
            path: dir.to_path_buf(),
            install_id: Some("TouchDesigner.2025.1".into()),
        }]
    }

    #[test]
    fn ids_encode_source_category_and_name() {
        assert_eq!(
            palette_id(PaletteSource::Builtin, Path::new("Tools/particlesGpu.tox")),
            "builtin:Tools/particlesGpu"
        );
        assert_eq!(
            palette_id(PaletteSource::User, Path::new("template.tox")),
            "user:template"
        );
        assert_eq!(
            split_id("builtin:UI/Basic Widgets/buttons"),
            ("UI/Basic Widgets".to_string(), "buttons".to_string())
        );
        assert_eq!(split_id("user:loose"), (String::new(), "loose".to_string()));
    }

    #[test]
    fn walk_finds_tox_recursively_and_skips_hidden_and_non_tox() {
        let dir = tempfile::tempdir().unwrap();
        fake_builtin(dir.path());
        let found = walk_palette_root(dir.path(), PaletteSource::Builtin);
        let ids: Vec<&str> = found.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "builtin:Tools/audioAnalysis",
                "builtin:Tools/particlesGpu",
                "builtin:UI/Basic Widgets/buttons",
                "builtin:template",
            ]
        );
    }

    #[test]
    fn scan_reports_add_update_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        fake_builtin(dir.path());
        let mut index = PaletteIndex::default();

        let first = scan_into(&mut index, &roots(dir.path()));
        assert_eq!(first.added, 4);
        assert_eq!(first.updated, 0);
        assert_eq!(first.total, 4);

        // Same tree again: no churn.
        let second = scan_into(&mut index, &roots(dir.path()));
        assert_eq!((second.added, second.updated, second.removed), (0, 0, 0));

        // Content change ⇒ update; deletion ⇒ removal.
        tox(dir.path(), "Tools/particlesGpu.tox", b"aaaaaaaaaaaa");
        fs::remove_file(dir.path().join("Tools/audioAnalysis.tox")).unwrap();
        let third = scan_into(&mut index, &roots(dir.path()));
        assert_eq!(third.updated, 1);
        assert_eq!(third.removed, 1);
        assert_eq!(third.total, 3);
        assert!(index.roots.len() == 1 && index.scanned_at.is_some());
    }

    #[test]
    fn scan_only_prunes_sources_it_actually_scanned() {
        let builtin = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        fake_builtin(builtin.path());
        tox(user.path(), "Mine/thing.tox", b"z");

        let mut index = PaletteIndex::default();
        let all = vec![
            PaletteRoot {
                source: PaletteSource::Builtin,
                path: builtin.path().to_path_buf(),
                install_id: None,
            },
            PaletteRoot {
                source: PaletteSource::User,
                path: user.path().to_path_buf(),
                install_id: None,
            },
        ];
        scan_into(&mut index, &all);
        assert_eq!(index.entries.len(), 5);

        // Rescan the user root alone — builtin entries must survive.
        let user_only = vec![PaletteRoot {
            source: PaletteSource::User,
            path: user.path().to_path_buf(),
            install_id: None,
        }];
        let report = scan_into(&mut index, &user_only);
        assert_eq!(report.removed, 0);
        assert_eq!(index.entries.len(), 5);
    }

    #[test]
    fn card_status_tracks_fingerprint_drift() {
        let dir = tempfile::tempdir().unwrap();
        fake_builtin(dir.path());
        let mut index = PaletteIndex::default();
        scan_into(&mut index, &roots(dir.path()));

        let id = "builtin:Tools/particlesGpu";
        let entry = index.entries.get_mut(id).unwrap();
        assert_eq!(entry.card_status(), CardStatus::Undescribed);

        entry.card = Some("cards/x.md".into());
        entry.card_fingerprint = Some(entry.fingerprint);
        assert_eq!(entry.card_status(), CardStatus::Described);

        tox(dir.path(), "Tools/particlesGpu.tox", b"changed content");
        let report = scan_into(&mut index, &roots(dir.path()));
        assert_eq!(report.stale, 1);
        assert_eq!(index.entries[id].card_status(), CardStatus::Stale);
    }

    #[test]
    fn glob_matches_stars_and_question_marks() {
        assert!(glob_match("builtin:Tools/*", "builtin:Tools/particlesGpu"));
        assert!(glob_match("*:Tools/*", "user:Tools/x"));
        assert!(glob_match("builtin:*", "builtin:UI/Basic Widgets/b"));
        assert!(glob_match("*particles*", "builtin:Tools/particlesGpu"));
        assert!(glob_match(
            "builtin:Tools/particlesGp?",
            "builtin:Tools/particlesGpu"
        ));
        assert!(!glob_match("builtin:UI/*", "builtin:Tools/particlesGpu"));
        assert!(!glob_match("builtin:Tools/x", "builtin:Tools/xy"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("a*b*c", "azzbzzc"));
        assert!(!glob_match("a*b*c", "azzbzz"));
    }

    #[test]
    fn ignore_globs_and_flags_both_count() {
        let dir = tempfile::tempdir().unwrap();
        fake_builtin(dir.path());
        let mut index = PaletteIndex::default();
        scan_into(&mut index, &roots(dir.path()));
        index.ignore.push("builtin:UI/*".into());
        assert!(index.is_ignored("builtin:UI/Basic Widgets/buttons"));
        assert!(!index.is_ignored("builtin:Tools/particlesGpu"));
        index
            .entries
            .get_mut("builtin:Tools/particlesGpu")
            .unwrap()
            .ignored = true;
        assert!(index.is_ignored("builtin:Tools/particlesGpu"));
    }

    #[test]
    fn selector_filters_by_category_status_and_ignore() {
        let dir = tempfile::tempdir().unwrap();
        fake_builtin(dir.path());
        let mut index = PaletteIndex::default();
        scan_into(&mut index, &roots(dir.path()));
        index.ignore.push("builtin:UI/*".into());

        let by_cat = Selector {
            category: Some("Tools".into()),
            ..Selector::default()
        };
        assert_eq!(by_cat.apply(&index).len(), 2);

        // Ignored entries are excluded by default, included on demand.
        let all = Selector::default();
        assert_eq!(all.apply(&index).len(), 3);
        let with_ignored = Selector {
            include_ignored: true,
            ..Selector::default()
        };
        assert_eq!(with_ignored.apply(&index).len(), 4);

        // Explicit ids bypass the ignore filter — asking for it is deliberate.
        let explicit = Selector {
            ids: vec!["builtin:UI/Basic Widgets/buttons".into()],
            ..Selector::default()
        };
        assert_eq!(explicit.apply(&index).len(), 1);

        let entry = index.entries.get_mut("builtin:Tools/particlesGpu").unwrap();
        entry.card = Some("cards/x.md".into());
        entry.card_fingerprint = Some(entry.fingerprint);
        let undescribed = Selector {
            status: SelectStatus::Undescribed,
            ..Selector::default()
        };
        assert!(!undescribed
            .apply(&index)
            .contains(&"builtin:Tools/particlesGpu".to_string()));
        let described = Selector {
            status: SelectStatus::Described,
            ..Selector::default()
        };
        assert_eq!(described.apply(&index), vec!["builtin:Tools/particlesGpu"]);
    }

    #[test]
    fn store_round_trips_index_and_cards() {
        let store_dir = tempfile::tempdir().unwrap();
        let palette = tempfile::tempdir().unwrap();
        fake_builtin(palette.path());
        let store = PaletteStore::new(store_dir.path().to_path_buf());

        assert_eq!(store.load().unwrap().entries.len(), 0);

        let mut index = PaletteIndex::default();
        scan_into(&mut index, &roots(palette.path()));
        let rel = store
            .write_card("builtin:Tools/particlesGpu", "# card body")
            .unwrap();
        index
            .entries
            .get_mut("builtin:Tools/particlesGpu")
            .unwrap()
            .card = Some(rel.clone());
        store.save(&index).unwrap();

        let reloaded = store.load().unwrap();
        assert_eq!(reloaded.entries.len(), 4);
        assert_eq!(store.read_card(&rel).unwrap(), "# card body");
        store.remove_card(&rel).unwrap();
        store.remove_card(&rel).unwrap(); // idempotent
    }

    #[test]
    fn corrupt_index_reports_store_error_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let store = PaletteStore::new(dir.path().to_path_buf());
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(store.index_path(), b"{ not json").unwrap();
        let err = store.load().unwrap_err();
        assert!(matches!(err, ProjectIoError::PaletteStore { .. }));

        fs::write(store.index_path(), br#"{"version":99,"entries":{}}"#).unwrap();
        let err = store.load().unwrap_err();
        assert!(format!("{err}").contains("re-run scan"));
    }

    #[test]
    fn card_slugs_are_filesystem_safe_and_distinct() {
        let a = card_slug("builtin:Tools/particlesGpu");
        let b = card_slug("builtin:Tools_particlesGpu");
        assert!(a.ends_with(".md") && !a.contains('/') && !a.contains(':'));
        assert_ne!(
            a, b,
            "sanitization alone would collide; the hash separates them"
        );
    }
}
