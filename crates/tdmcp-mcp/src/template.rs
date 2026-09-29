//! Jinja template engine for skill files.
//!
//! Templates under `skills/templates/` use `{{ skill("id") }}` and
//! `{{ skill_read("id") }}` to reference other skill cards. At render time
//! the engine resolves these to either MCP resource URIs or filesystem-
//! relative Markdown links depending on [`RenderMode`].
//!
//! The [`Catalog`] is loaded from the embedded `skills/MANIFEST.yaml` and
//! maps each id to its output path, title, and description.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use include_dir::Dir;
use minijinja::{context, Environment, Error, State};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Manifest / catalog
// ---------------------------------------------------------------------------

/// One entry from `skills/MANIFEST.yaml`.
#[derive(Debug, Clone, Deserialize)]
struct ManifestEntry {
    template_path: String,
    output_path: String,
    title: String,
    description: String,
    /// Tool ids this card is only useful with (D5 capability gate).
    ///
    /// Empty means the card depends on no tool and is always available. A
    /// non-empty list means the card must not be handed to a caller whose
    /// backend does not register those tools — teaching a release-plan tool
    /// name to a build that cannot honour it is the failure this prevents.
    #[serde(default)]
    requires: Vec<String>,
}

/// Flat catalog of all known skill ids.
#[derive(Debug, Clone)]
pub struct Catalog {
    entries: Vec<CatalogEntry>,
    by_id: HashMap<String, usize>,
}

/// One catalog entry (resolved from MANIFEST).
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    /// Stable skill id (used in URI and `{{ skill() }}` calls).
    pub id: String,
    /// Rendered output path relative to the skills root.
    pub output_path: String,
    /// Template path relative to `skills/templates/`.
    pub template_path: String,
    /// Short human-readable title.
    pub title: String,
    /// One-line description.
    pub description: String,
    /// Tool ids this card is only useful with (D5 capability gate).
    pub requires: Vec<String>,
}

impl Catalog {
    /// Parse the embedded `skills/MANIFEST.yaml`.
    pub fn from_manifest_yaml(yaml: &str) -> Result<Self, String> {
        let raw: HashMap<String, ManifestEntry> = serde_yaml::from_str(yaml)
            .map_err(|e| format!("failed to parse MANIFEST.yaml: {e}"))?;

        let mut entries = Vec::with_capacity(raw.len());
        let mut by_id = HashMap::with_capacity(raw.len());

        for (id, me) in &raw {
            if by_id.contains_key(id) {
                return Err(format!("duplicate skill id in MANIFEST: {id}"));
            }
            let idx = entries.len();
            by_id.insert(id.clone(), idx);
            entries.push(CatalogEntry {
                id: id.clone(),
                output_path: me.output_path.clone(),
                template_path: me.template_path.clone(),
                title: me.title.clone(),
                description: me.description.clone(),
                requires: me.requires.clone(),
            });
        }

        Ok(Self { entries, by_id })
    }

    /// Look up a skill by its id.
    pub fn get(&self, id: &str) -> Option<&CatalogEntry> {
        self.by_id.get(id).map(|&i| &self.entries[i])
    }

    /// Iterate all catalog entries.
    pub fn iter(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.iter()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalog is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Capability gate (D5)
// ---------------------------------------------------------------------------

/// The set of tool ids a running backend can actually honour.
///
/// This is deliberately driven by what the *backend reports*, not by the
/// compile-time [`ToolName::ALL`](crate::tools::ToolName) list: the bridge that
/// actually serves a session can be an older build than this binary, and a
/// card that names a tool the running backend never registered is a promise
/// nothing will keep.
///
/// An unknown capability set is represented by `None`, which means "not yet
/// observed" and gates nothing. That is the honest default: withholding every
/// gated card because a probe failed would be as misleading as the opposite.
#[derive(Debug, Clone, Default)]
pub struct CapabilityGate {
    available: Option<Vec<String>>,
}

impl CapabilityGate {
    /// No observed capabilities: every card is available.
    #[must_use]
    pub fn unobserved() -> Self {
        Self { available: None }
    }

    /// Build from the tool ids the backend actually advertised.
    #[must_use]
    pub fn from_tools<I, S>(tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut available: Vec<String> = tools.into_iter().map(Into::into).collect();
        available.sort();
        Self {
            available: Some(available),
        }
    }

    /// Whether capabilities have actually been observed.
    #[must_use]
    pub fn is_observed(&self) -> bool {
        self.available.is_some()
    }

    /// Whether one tool id is available.
    ///
    /// An unobserved gate answers `true` for everything, so a card is never
    /// hidden merely because a probe did not run.
    #[must_use]
    pub fn has(&self, tool: &str) -> bool {
        match &self.available {
            None => true,
            Some(list) => list.iter().any(|t| t == tool),
        }
    }

    /// Whether a card may be handed to a caller.
    ///
    /// A card with no `requires` list is always available: it teaches nothing
    /// about a tool that could be missing.
    #[must_use]
    pub fn allows(&self, entry: &CatalogEntry) -> bool {
        entry.requires.iter().all(|tool| self.has(tool))
    }

    /// Which of a card's required tools this gate does not have.
    #[must_use]
    pub fn missing_for<'e>(&self, entry: &'e CatalogEntry) -> Vec<&'e str> {
        entry
            .requires
            .iter()
            .filter(|t| !self.has(t))
            .map(String::as_str)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Render mode
// ---------------------------------------------------------------------------

/// Output mode for template rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// MCP resource mode: `skill("x")` → `` `tdmcp://docs/x` ``.
    Mcp,
    /// Filesystem mode: `skill("x")` → `` [`x`](./path/to/x.md) ``.
    FileSystem,
}

// ---------------------------------------------------------------------------
// Path utilities
// ---------------------------------------------------------------------------

fn relative_path(from: &str, to: &str) -> String {
    let from = Path::new(from);
    let to = Path::new(to);

    let from_dir = from.parent().unwrap_or_else(|| Path::new(""));

    let from_parts: Vec<&str> = from_dir
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    let to_parts: Vec<&str> = to
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    let common = from_parts
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let up_count = from_parts.len() - common;
    let mut segments: Vec<&str> = Vec::new();

    segments.extend(std::iter::repeat_n("..", up_count));
    for part in &to_parts[common..] {
        segments.push(part);
    }

    let joined = segments.join("/");

    if joined.is_empty() {
        String::from(".")
    } else if !joined.starts_with('.') {
        format!("./{joined}")
    } else {
        joined
    }
}

/// Resolve the render mode from template state.
fn mode_from_state(state: &State) -> RenderMode {
    match state
        .lookup("_mode")
        .and_then(|v| v.as_str().map(|s| s.to_string()))
    {
        Some(s) if s == "filesystem" => RenderMode::FileSystem,
        _ => RenderMode::Mcp,
    }
}

/// Resolve the current output path from template state.
fn current_path_from_state(state: &State) -> Option<String> {
    state
        .lookup("_current_path")
        .and_then(|v| v.as_str().map(String::from))
}

// ---------------------------------------------------------------------------
// Template engine
// ---------------------------------------------------------------------------

/// Renders skill templates from the embedded `skills/templates/` tree.
pub struct TemplateEngine {
    catalog: Arc<Catalog>,
    env: Environment<'static>,
    templates: HashMap<String, String>,
    /// Which cards this backend may hand out (D5).
    gate: CapabilityGate,
}

impl TemplateEngine {
    /// Build the engine from a parsed [`Catalog`] and the embedded templates
    /// directory (`skills/templates/`).
    pub fn new(catalog: Catalog, templates_dir: &'static Dir<'_>) -> Result<Self, String> {
        Self::with_gate(catalog, templates_dir, CapabilityGate::unobserved())
    }

    /// Build the engine with an observed [`CapabilityGate`].
    ///
    /// Cards whose `requires` list names a tool the gate does not have are
    /// then neither renderable nor linkable, so a caller cannot be walked to
    /// a tool name this backend cannot honour.
    pub fn with_gate(
        catalog: Catalog,
        templates_dir: &'static Dir<'_>,
        gate: CapabilityGate,
    ) -> Result<Self, String> {
        let mut templates = HashMap::new();
        collect_templates(templates_dir, &mut templates);

        for entry in catalog.iter() {
            if !templates.contains_key(&entry.template_path) {
                return Err(format!(
                    "template missing for '{}': {}",
                    entry.id, entry.template_path
                ));
            }
        }

        let catalog = Arc::new(catalog);
        let env = build_env(Arc::clone(&catalog), gate.clone());

        Ok(Self {
            catalog,
            env,
            templates,
            gate,
        })
    }

    /// The capability gate in force.
    #[must_use]
    pub fn gate(&self) -> &CapabilityGate {
        &self.gate
    }

    /// Catalog entries this backend may hand out.
    ///
    /// A gated card is omitted rather than rendered with a warning: a card
    /// that says "you could use compare_nodes if the backend had it" still
    /// teaches the name.
    pub fn available_entries(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.catalog.iter().filter(|e| self.gate.allows(e))
    }

    /// Why a card is unavailable, for an honest refusal.
    fn gate_refusal(&self, id: &str) -> Option<String> {
        let entry = self.catalog.get(id)?;
        if self.gate.allows(entry) {
            return None;
        }
        let missing = self.gate.missing_for(entry);
        Some(format!(
            "skill '{id}' needs tool(s) this backend does not register: {}; \
             it is withheld rather than rendered, because a card naming an \
             unavailable tool teaches a promise nothing will keep",
            missing.join(", ")
        ))
    }

    /// Test-only: construct an engine from pre-built components (bypasses
    /// `include_dir` checks).
    #[doc(hidden)]
    pub fn new_for_test(
        catalog: Catalog,
        env: Environment<'static>,
        templates: HashMap<String, String>,
    ) -> Self {
        Self {
            catalog: Arc::new(catalog),
            env,
            templates,
            gate: CapabilityGate::unobserved(),
        }
    }

    /// Render one template by its catalog id in the given mode.
    pub fn render(&self, id: &str, mode: RenderMode) -> Result<String, String> {
        if let Some(refusal) = self.gate_refusal(id) {
            return Err(refusal);
        }
        let entry = self.catalog.get(id).ok_or_else(|| {
            // Agents self-correct from the available-ids list — never bare.
            let mut ids: Vec<&str> = self.catalog.iter().map(|e| e.id.as_str()).collect();
            ids.sort_unstable();
            const CAP: usize = 8;
            let list = if ids.len() <= CAP {
                ids.join(", ")
            } else {
                format!("{}, … (+{} more)", ids[..CAP].join(", "), ids.len() - CAP)
            };
            format!(
                "unknown skill id: {id} — available: {list}; call resources/list for the full index"
            )
        })?;

        let source = self
            .templates
            .get(&entry.template_path)
            .ok_or_else(|| format!("template not loaded: {}", entry.template_path))?;

        let tmpl = self
            .env
            .template_from_str(source)
            .map_err(|e| format!("failed to parse template {}: {e}", entry.template_path))?;

        let mode_str = match mode {
            RenderMode::Mcp => "mcp",
            RenderMode::FileSystem => "filesystem",
        };

        let result = tmpl
            .render(context! {
                _mode => mode_str,
                _current_path => &entry.output_path,
            })
            .map_err(|e| format!("failed to render template {}: {e}", entry.template_path))?;

        Ok(result)
    }

    /// Render all catalog entries the gate allows, in the given mode.
    ///
    /// Returns a `Vec` of `(output_path, content)` pairs. A gated-out card is
    /// simply absent — it is not rendered with a caveat, because a caveat
    /// still names the tool.
    pub fn render_all(&self, mode: RenderMode) -> Result<Vec<(String, String)>, String> {
        let mut results = Vec::with_capacity(self.catalog.len());
        for entry in self.available_entries() {
            let content = self.render(&entry.id, mode)?;
            results.push((entry.output_path.clone(), content));
        }
        Ok(results)
    }

    /// Borrow the catalog.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }
}

/// Build a Minijinja environment with `skill` / `skill_read` functions that
/// capture the catalog by `Arc`.
fn build_env(catalog: Arc<Catalog>, gate: CapabilityGate) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| minijinja::AutoEscape::None);

    let cat = Arc::clone(&catalog);
    let read_gate = gate.clone();
    let skill_gate = gate.clone();
    let has_cap = gate.clone();
    // A capability predicate, so a template can omit a whole row or section
    // rather than render an empty one. `skill_opt` blanks the link but leaves
    // the surrounding text, which reads as a formatting fault inside a table.
    env.add_function(
        "has_capability",
        move |_state: &State, tool: String| -> bool { has_cap.has(&tool) },
    );

    env.add_function(
        "skill",
        move |state: &State, id: String| -> Result<String, Error> {
            let mode = mode_from_state(state);
            let current_path = current_path_from_state(state);

            let entry = cat.get(&id).ok_or_else(|| {
                Error::new(
                    minijinja::ErrorKind::UnknownMethod,
                    format!("unknown skill id: {id}"),
                )
            })?;

            // A link to a withheld card is as much a leak as handing over the
            // card: the caller would fetch a URI this backend will refuse.
            if !skill_gate.allows(entry) {
                return Err(gate_refusal_error(entry, &skill_gate));
            }

            Ok(match mode {
                RenderMode::Mcp => format!("`tdmcp://docs/{}`", entry.id),
                RenderMode::FileSystem => {
                    let rel = match &current_path {
                        Some(cp) => relative_path(cp, &entry.output_path),
                        None => format!("./{}", entry.output_path),
                    };
                    format!("[`{}`]({rel})", entry.id)
                }
            })
        },
    );

    // The gate-aware variant, for OPTIONAL links. A gated-out card here
    // renders as nothing, so an index that lists optional overlays survives a
    // backend that cannot honour one. `skill()` stays strict on purpose: a
    // required link to a missing card should fail the render loudly rather
    // than leave a hole the reader cannot explain.
    let opt_cat = Arc::clone(&catalog);
    let opt_gate = gate.clone();
    env.add_function(
        "skill_opt",
        move |state: &State, id: String| -> Result<String, Error> {
            let Some(entry) = opt_cat.get(&id) else {
                return Err(Error::new(
                    minijinja::ErrorKind::UnknownMethod,
                    format!("unknown skill id: {id}"),
                ));
            };
            if !opt_gate.allows(entry) {
                return Ok(String::new());
            }
            let mode = mode_from_state(state);
            let current_path = current_path_from_state(state);
            Ok(match mode {
                RenderMode::Mcp => format!("`tdmcp://docs/{}`", entry.id),
                RenderMode::FileSystem => {
                    let rel = match &current_path {
                        Some(cp) => relative_path(cp, &entry.output_path),
                        None => format!("./{}", entry.output_path),
                    };
                    format!("[`{}`]({rel})", entry.id)
                }
            })
        },
    );

    env.add_function(
        "skill_read",
        move |state: &State, id: String| -> Result<String, Error> {
            let mode = mode_from_state(state);
            let current_path = current_path_from_state(state);

            let entry = catalog.get(&id).ok_or_else(|| {
                Error::new(
                    minijinja::ErrorKind::UnknownMethod,
                    format!("unknown skill id: {id}"),
                )
            })?;

            // Same gate as `skill`: a "read this card" pointer to a withheld
            // card teaches the name just as much as a link does.
            if !read_gate.allows(entry) {
                return Err(gate_refusal_error(entry, &read_gate));
            }

            Ok(match mode {
                RenderMode::Mcp => format!("`resources/read` `tdmcp://docs/{}`", entry.id),
                RenderMode::FileSystem => {
                    let rel = match &current_path {
                        Some(cp) => relative_path(cp, &entry.output_path),
                        None => format!("./{}", entry.output_path),
                    };
                    format!("see [`{}.md`]({rel})", entry.id)
                }
            })
        },
    );

    env
}

/// The error a gated-out card produces inside a template.
fn gate_refusal_error(entry: &CatalogEntry, gate: &CapabilityGate) -> Error {
    Error::new(
        minijinja::ErrorKind::UnknownMethod,
        format!(
            "skill '{}' is unavailable: this backend does not register {}; \
             it is not linked so the name is not taught",
            entry.id,
            gate.missing_for(entry).join(", ")
        ),
    )
}

/// Recursively collect `.jinja.md` files from an embedded dir.
///
/// `DirEntry::path()` is already archive-relative (full path from the root),
/// so no manual prefixing is needed.
fn collect_templates(dir: &Dir<'_>, out: &mut HashMap<String, String>) {
    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(sub) => {
                collect_templates(sub, out);
            }
            include_dir::DirEntry::File(file) => {
                let rel = file.path().to_str().unwrap_or("").replace('\\', "/");
                if !rel.ends_with(".jinja.md") {
                    continue;
                }
                if let Ok(text) = std::str::from_utf8(file.contents()) {
                    out.insert(rel, text.to_owned());
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    fn test_catalog() -> Catalog {
        let yaml = r#"
operate:
  template_path: touchdesigner/SKILL.jinja.md
  output_path: touchdesigner/SKILL.md
  title: TouchDesigner operate
  description: Umbrella skill
python-api:
  template_path: touchdesigner/reference/python-api.jinja.md
  output_path: touchdesigner/reference/python-api.md
  title: TD Python API
  description: Mandatory before execute_python
glsl:
  template_path: touchdesigner/reference/glsl.jinja.md
  output_path: touchdesigner/reference/glsl.md
  title: TD GLSL dialect
  description: GLSL TOP/MAT dialect
primer/cook-and-families:
  template_path: touchdesigner/primer/cook-and-families.jinja.md
  output_path: touchdesigner/primer/cook-and-families.md
  title: "Primer: cook and families"
  description: Pull cook model
"#;
        Catalog::from_manifest_yaml(yaml).expect("test catalog")
    }

    /// A catalog carrying one gated card, for the capability-gate tests.
    fn gated_catalog() -> Catalog {
        let yaml = r#"
operate:
  template_path: touchdesigner/SKILL.jinja.md
  output_path: touchdesigner/SKILL.md
  title: TouchDesigner operate
  description: Umbrella skill
python-api:
  template_path: touchdesigner/reference/python-api.jinja.md
  output_path: touchdesigner/reference/python-api.md
  title: TD Python API
  description: Mandatory before execute_python
delta-guarded-writes:
  template_path: touchdesigner/reference/delta-guarded-writes.jinja.md
  output_path: touchdesigner/reference/delta-guarded-writes.md
  title: Delta and guarded writes
  description: Stamped baseline and guarded writes
  requires:
    - compare_nodes
"#;
        Catalog::from_manifest_yaml(yaml).expect("gated catalog")
    }

    fn engine_for(cat: Catalog, source: &str) -> TemplateEngine {
        engine_with_gate(cat, source, CapabilityGate::unobserved())
    }

    fn engine_with_gate(cat: Catalog, source: &str, gate: CapabilityGate) -> TemplateEngine {
        let mut templates = HashMap::new();
        for entry in cat.iter() {
            templates.insert(entry.template_path.clone(), source.to_string());
        }

        let cat = Arc::new(cat);
        let env = build_env(Arc::clone(&cat), gate.clone());

        TemplateEngine {
            catalog: cat,
            env,
            templates,
            gate,
        }
    }

    // -- relative_path -------------------------------------------------------

    #[test]
    fn relative_path_same_dir() {
        assert_eq!(
            relative_path(
                "touchdesigner/reference/glsl.md",
                "touchdesigner/reference/python-api.md"
            ),
            "./python-api.md"
        );
    }

    #[test]
    fn relative_path_subdir() {
        assert_eq!(
            relative_path(
                "touchdesigner/SKILL.md",
                "touchdesigner/reference/python-api.md"
            ),
            "./reference/python-api.md"
        );
    }

    #[test]
    fn relative_path_parent_dir() {
        assert_eq!(
            relative_path(
                "touchdesigner/reference/glsl.md",
                "touchdesigner/primer/cook-and-families.md"
            ),
            "../primer/cook-and-families.md"
        );
    }

    // -- Catalog -------------------------------------------------------------

    #[test]
    fn catalog_from_yaml() {
        let cat = test_catalog();
        assert_eq!(cat.len(), 4);
        assert!(cat.get("operate").is_some());
        assert!(cat.get("nope").is_none());
    }

    // -- Render MCP mode -----------------------------------------------------

    #[test]
    fn skill_mcp_mode() {
        let engine = engine_for(test_catalog(), "See {{ skill(\"python-api\") }}.");
        let result = engine.render("operate", RenderMode::Mcp).unwrap();
        assert_eq!(result, "See `tdmcp://docs/python-api`.");
    }

    #[test]
    fn skill_read_mcp_mode() {
        let engine = engine_for(test_catalog(), "Before: {{ skill_read(\"python-api\") }}.");
        let result = engine.render("operate", RenderMode::Mcp).unwrap();
        assert_eq!(
            result,
            "Before: `resources/read` `tdmcp://docs/python-api`."
        );
    }

    // -- Render FS mode ------------------------------------------------------

    #[test]
    fn skill_fs_subdir() {
        let engine = engine_for(test_catalog(), "{{ skill(\"python-api\") }}");
        // From SKILL.md → reference/python-api.md
        let result = engine.render("operate", RenderMode::FileSystem).unwrap();
        assert_eq!(result, "[`python-api`](./reference/python-api.md)");
    }

    #[test]
    fn skill_fs_sibling() {
        let engine = engine_for(test_catalog(), "{{ skill(\"python-api\") }}");
        // From glsl.md → python-api.md (same dir)
        let result = engine.render("glsl", RenderMode::FileSystem).unwrap();
        assert_eq!(result, "[`python-api`](./python-api.md)");
    }

    #[test]
    fn skill_read_fs_mode() {
        let engine = engine_for(test_catalog(), "Before: {{ skill_read(\"python-api\") }}.");
        let result = engine.render("operate", RenderMode::FileSystem).unwrap();
        assert_eq!(
            result,
            "Before: see [`python-api.md`](./reference/python-api.md)."
        );
    }

    // -- Errors --------------------------------------------------------------

    #[test]
    fn unknown_skill_errors() {
        let engine = engine_for(test_catalog(), "{{ skill(\"bogus\") }}");
        let result = engine.render("operate", RenderMode::Mcp);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("bogus"));
    }

    #[test]
    fn unknown_id_errors() {
        let engine = engine_for(test_catalog(), "ok");
        let result = engine.render("nope", RenderMode::Mcp);
        assert!(result.is_err());
    }

    // -- Capability gate (D5) -----------------------------------------------

    #[test]
    fn an_unobserved_gate_withholds_nothing() {
        // The honest default: a probe that never ran must not hide every
        // gated card, which would be as misleading as the opposite.
        let engine = engine_with_gate(gated_catalog(), "body", CapabilityGate::unobserved());
        assert!(!engine.gate().is_observed());
        assert!(engine
            .render("delta-guarded-writes", RenderMode::Mcp)
            .is_ok());
        assert_eq!(engine.render_all(RenderMode::Mcp).unwrap().len(), 3);
    }

    #[test]
    fn a_gated_card_is_withheld_when_its_tool_is_absent() {
        let engine = engine_with_gate(
            gated_catalog(),
            "body",
            CapabilityGate::from_tools(["inspect", "mutate_nodes"]),
        );
        let err = engine
            .render("delta-guarded-writes", RenderMode::Mcp)
            .expect_err("a card naming an unavailable tool must not render");
        assert!(err.contains("compare_nodes"), "{err}");
        // The rest of the pack still renders: withholding one card must not
        // take the docs down with it.
        assert!(engine.render("operate", RenderMode::Mcp).is_ok());
        assert!(engine.render("python-api", RenderMode::Mcp).is_ok());
    }

    #[test]
    fn a_gated_card_renders_once_its_tool_is_present() {
        let engine = engine_with_gate(
            gated_catalog(),
            "body",
            CapabilityGate::from_tools(["inspect", "compare_nodes"]),
        );
        assert!(engine
            .render("delta-guarded-writes", RenderMode::Mcp)
            .is_ok());
        assert_eq!(engine.render_all(RenderMode::Mcp).unwrap().len(), 3);
    }

    #[test]
    fn a_gated_card_is_absent_from_render_all_rather_than_rendered_with_a_caveat() {
        // A card that says "use compare_nodes if you have it" still teaches
        // the name, which is the whole thing the gate exists to prevent.
        let engine = engine_with_gate(
            gated_catalog(),
            "body",
            CapabilityGate::from_tools(["inspect"]),
        );
        let outputs: Vec<String> = engine
            .render_all(RenderMode::Mcp)
            .unwrap()
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        assert!(!outputs.iter().any(|p| p.contains("delta-guarded-writes")));
        assert!(outputs.iter().any(|p| p.contains("python-api")));
    }

    #[test]
    fn a_strict_link_to_a_withheld_card_fails_the_render() {
        // `skill()` is strict on purpose: a required link to a missing card
        // should fail loudly rather than leave a hole the reader cannot
        // explain.
        let engine = engine_with_gate(
            gated_catalog(),
            "See {{ skill(\"delta-guarded-writes\") }}.",
            CapabilityGate::from_tools(["inspect"]),
        );
        let err = engine.render("operate", RenderMode::Mcp).unwrap_err();
        assert!(err.contains("compare_nodes"), "{err}");
    }

    #[test]
    fn an_optional_link_to_a_withheld_card_renders_empty_instead_of_failing() {
        // This is what lets an index row survive on a backend that cannot
        // honour the overlay.
        let engine = engine_with_gate(
            gated_catalog(),
            "See {{ skill_opt(\"delta-guarded-writes\") }}.",
            CapabilityGate::from_tools(["inspect"]),
        );
        let out = engine.render("operate", RenderMode::Mcp).unwrap();
        assert_eq!(out, "See .");
        assert!(!out.contains("delta-guarded-writes"));
    }

    #[test]
    fn an_optional_link_still_resolves_when_the_capability_is_there() {
        let engine = engine_with_gate(
            gated_catalog(),
            "See {{ skill_opt(\"delta-guarded-writes\") }}.",
            CapabilityGate::from_tools(["compare_nodes"]),
        );
        let out = engine.render("operate", RenderMode::Mcp).unwrap();
        assert!(out.contains("tdmcp://docs/delta-guarded-writes"), "{out}");
    }

    #[test]
    fn an_optional_link_to_an_unknown_id_is_still_an_error() {
        // Gating must not turn a typo into a silently empty cell.
        let engine = engine_with_gate(
            gated_catalog(),
            "{{ skill_opt(\"nope\") }}",
            CapabilityGate::unobserved(),
        );
        assert!(engine.render("operate", RenderMode::Mcp).is_err());
    }

    #[test]
    fn the_capability_predicate_lets_a_template_omit_a_whole_row() {
        let body = "{% if has_capability(\"compare_nodes\") %}\n| delta |\n{% endif %}";
        let without = engine_with_gate(
            gated_catalog(),
            body,
            CapabilityGate::from_tools(["inspect"]),
        );
        assert_eq!(without.render("operate", RenderMode::Mcp).unwrap(), "");
        let with = engine_with_gate(
            gated_catalog(),
            body,
            CapabilityGate::from_tools(["compare_nodes"]),
        );
        assert!(with
            .render("operate", RenderMode::Mcp)
            .unwrap()
            .contains("delta"));
    }

    #[test]
    fn a_card_needing_every_one_of_several_tools_is_withheld_if_any_is_missing() {
        let yaml = r#"
both:
  template_path: both.jinja.md
  output_path: both.md
  title: Both
  description: needs two
  requires:
    - compare_nodes
    - sample_data
"#;
        let cat = Catalog::from_manifest_yaml(yaml).expect("catalog");
        let partial = engine_with_gate(cat, "body", CapabilityGate::from_tools(["compare_nodes"]));
        let entry = partial.catalog().get("both").expect("entry");
        assert!(!partial.gate().allows(entry));
        assert_eq!(partial.gate().missing_for(entry), vec!["sample_data"]);
    }

    #[test]
    fn the_shipped_pack_gates_the_overlay_card_on_compare_nodes() {
        // The real pack, not a fixture: this is the assertion that keeps the
        // manifest's `requires` honest as tools are added and removed.
        let cat = Catalog::from_manifest_yaml(crate::MANIFEST_YAML).expect("manifest");
        let entry = cat
            .get("delta-guarded-writes")
            .expect("delta-guarded-writes is registered");
        assert_eq!(entry.requires, vec!["compare_nodes".to_string()]);
        assert!(cat.get("operate").expect("operate").requires.is_empty());
    }

    /// The real shipped pack under a real gate that lacks `compare_nodes`.
    ///
    /// This is the assertion that actually matters: every OTHER card must
    /// still render, the overlay must be gone, and the *rendered text* of
    /// every other card must not mention the tool at all. A gate that merely
    /// refused the card would still leak the name through the two cards that
    /// link to it, which is the failure the gate exists to prevent.
    #[test]
    fn the_shipped_pack_never_teaches_a_tool_the_backend_lacks() {
        let gate = CapabilityGate::from_tools([
            "fleet",
            "execute_python",
            "inspect",
            "mutate_nodes",
            "capture",
            "record",
            "api_help",
            "editor_context",
            "describe_tools",
            "td_installs",
            "dialogs",
            "spawn_td",
            "kill_td",
            "project_unpack",
            "project_pack",
            "project_lint",
            "project_install_bridge",
            "palette_index",
            "palette_probe",
        ]);
        let cat = Catalog::from_manifest_yaml(crate::MANIFEST_YAML).expect("manifest");
        let engine =
            TemplateEngine::with_gate(cat, &crate::TEMPLATES, gate).expect("templates under gate");

        assert!(engine
            .render("delta-guarded-writes", RenderMode::Mcp)
            .is_err());

        for entry in engine.available_entries() {
            for mode in [RenderMode::Mcp, RenderMode::FileSystem] {
                let rendered = engine.render(&entry.id, mode).expect("render");
                assert!(
                    !rendered.contains("delta-guarded-writes"),
                    "{} ({mode:?}) still links the withheld card",
                    entry.id
                );
                assert!(
                    !rendered.contains("compare_nodes"),
                    "{} ({mode:?}) still names the withheld tool",
                    entry.id
                );
            }
        }

        // The two cards that used to link it must still render, with the row
        // gone rather than left with an empty cell.
        let umbrella = engine
            .render("operate", RenderMode::FileSystem)
            .expect("umbrella");
        assert!(!umbrella.contains("Confirm what changed"));
        let index = engine
            .render("reference-index", RenderMode::FileSystem)
            .expect("index");
        assert!(!index.contains("Confirming a change"));
        for line in index.lines().filter(|l| l.starts_with('|')) {
            assert!(
                !line.ends_with("|  |"),
                "index row left with an empty cell: {line}"
            );
        }
    }

    /// The same pack, WITH the capability: the overlay renders and both cards
    /// link it. Guards against a gate that silently withholds everything.
    #[test]
    fn the_shipped_pack_renders_the_overlay_once_the_capability_exists() {
        let gate = CapabilityGate::from_tools(["inspect", "compare_nodes"]);
        let cat = Catalog::from_manifest_yaml(crate::MANIFEST_YAML).expect("manifest");
        let engine =
            TemplateEngine::with_gate(cat, &crate::TEMPLATES, gate).expect("templates under gate");

        assert!(engine
            .render("delta-guarded-writes", RenderMode::Mcp)
            .is_ok());
        let umbrella = engine
            .render("operate", RenderMode::FileSystem)
            .expect("umbrella");
        assert!(umbrella.contains("delta-guarded-writes"), "{umbrella}");
        let index = engine
            .render("reference-index", RenderMode::FileSystem)
            .expect("index");
        assert!(index.contains("delta-guarded-writes"), "{index}");
    }

    static EMPTY_DIR: Dir<'static> = Dir::new("", &[]);

    #[test]
    fn template_missing_for_entry() {
        let cat = test_catalog();
        let result = TemplateEngine::new(cat, &EMPTY_DIR);
        assert!(result.is_err());
        assert!(result.err().unwrap().contains("template missing"));
    }

    // -- render_all ----------------------------------------------------------

    #[test]
    fn render_all_mcp() {
        let engine = engine_for(test_catalog(), "");
        let results = engine.render_all(RenderMode::Mcp).unwrap();
        assert_eq!(results.len(), 4);
    }

    #[test]
    fn guidance_registry_and_both_render_modes_cover_migration() {
        let registry: serde_json::Value =
            serde_json::from_str(include_str!("../../../skills/GUIDANCE.json"))
                .expect("guidance authoring registry");
        let cat = Catalog::from_manifest_yaml(crate::MANIFEST_YAML).expect("manifest");
        let engine = TemplateEngine::new(cat.clone(), &crate::TEMPLATES).expect("templates");
        let cards = registry["cards"].as_object().expect("cards");
        assert_eq!(
            cards.len(),
            cat.len(),
            "every card needs explicit disposition"
        );
        let legacy = registry["legacyIds"].as_array().expect("legacy IDs");
        assert_eq!(
            legacy.len(),
            33,
            "preserve the original public resource set"
        );
        for id in legacy {
            assert!(cat.get(id.as_str().expect("id")).is_some());
        }
        let owners = registry["ruleOwners"].as_object().expect("rule ownership");
        assert_eq!(owners.len(), 20);
        for index in 1..=20 {
            let owner = owners[&format!("G{index:02}")].as_str().expect("owner");
            assert!(cat.get(owner).is_some(), "unknown canonical owner: {owner}");
        }
        for entry in cat.iter() {
            let meta = &cards[&entry.id];
            assert_eq!(
                meta["trigger"].as_str(),
                Some(entry.description.as_str()),
                "manifest/authoring trigger drift: {}",
                entry.id
            );
            for field in [
                "trigger",
                "nonTrigger",
                "audience",
                "kind",
                "migration",
                "evidenceScope",
            ] {
                assert!(
                    !meta[field].as_str().expect(field).is_empty(),
                    "{}: {field}",
                    entry.id
                );
            }
            assert!(cat
                .get(meta["canonicalOwner"].as_str().expect("canonical owner"))
                .is_some());
            for mode in [RenderMode::Mcp, RenderMode::FileSystem] {
                let rendered = engine.render(&entry.id, mode).expect("render");
                if matches!(entry.id.as_str(), "operate" | "python-api") {
                    eprintln!(
                        "guidance-size id={} mode={mode:?} chars={} utf8Bytes={} (not tokens)",
                        entry.id,
                        rendered.chars().count(),
                        rendered.len()
                    );
                }
                assert!(!rendered.contains("Editorial preview"));
                assert!(!rendered.contains("{{ skill("));
                assert!(!rendered.contains(".jinja.md"));
                assert!(rendered.contains("**Canonical:**"));
            }
        }
        let entry = engine.render("python-api", RenderMode::Mcp).unwrap();
        let reference = engine.render("python-reference", RenderMode::Mcp).unwrap();
        for heading in reference.lines().filter(|line| line.starts_with("## ")) {
            assert!(
                entry.contains(heading),
                "lost old Python heading: {heading}"
            );
        }
        assert!(
            entry.len() < reference.len(),
            "execution contract should be smaller than reference"
        );
        assert!(entry.contains("tdmcp://docs/python-reference"));
    }

    // -- template lint (whole embedded pack) ---------------------------------

    /// Cross-reference discipline for the shipped pack: no hardcoded `.md`
    /// links, no vague "mcp skill" phrases, and every `skill()`/`skill_read()`
    /// id must exist in the MANIFEST. Hardcoded links break MCP mode, where
    /// cards are served as `tdmcp://docs/<id>` resources, not files.
    #[test]
    fn template_pack_cross_references_are_well_formed() {
        let cat = Catalog::from_manifest_yaml(crate::MANIFEST_YAML).expect("manifest");

        let mut templates = HashMap::new();
        collect_templates(&crate::TEMPLATES, &mut templates);
        assert!(
            templates.len() >= cat.len(),
            "embedded template tree smaller than catalog"
        );

        let mut problems: Vec<String> = Vec::new();
        for (path, source) in &templates {
            // Hardcoded markdown link to a .md file (renderer-produced links are
            // injected at render time, so any `.md)` in source is authored).
            if source.contains(".md)") {
                problems.push(format!("{path}: hardcoded `.md)` link"));
            }
            // Vague pointer to "the mcp skill" instead of a skill() id.
            if source.contains("mcp skill") {
                problems.push(format!("{path}: vague \"mcp skill\" phrase"));
            }
            // One skeleton: every card ends with `## Related` + a canonical
            // line. The umbrella routes and needs neither.
            if !path.ends_with("SKILL.jinja.md") {
                if !source.contains("## Related") {
                    problems.push(format!("{path}: missing `## Related` section"));
                }
                if !source.contains("**Canonical:**") {
                    problems.push(format!("{path}: missing `**Canonical:**` line"));
                }
            }
            // Every referenced id must resolve.
            for captures in ["skill(", "skill_read(", "skill_opt("] {
                let mut rest = source.as_str();
                while let Some(idx) = rest.find(captures) {
                    let after = &rest[idx + captures.len()..];
                    let Some(open) = after.find('"') else { break };
                    let inner = &after[open + 1..];
                    let Some(close) = inner.find('"') else { break };
                    let id = &inner[..close];
                    if cat.get(id).is_none() {
                        problems.push(format!("{path}: unknown skill id {id:?}"));
                    }
                    rest = &inner[close..];
                }
            }
        }
        assert!(
            problems.is_empty(),
            "template cross-reference problems:\n{}",
            problems.join("\n")
        );
    }

    /// Every card must be reachable from the `operate` umbrella by following
    /// `skill()` / `skill_read()` references. A card in the MANIFEST that
    /// nothing links to is served by `resources/list` but invisible in
    /// filesystem-export mode, where `SKILL.md` is the only entry point --
    /// which is exactly how `project-io` / `lifecycle` / `popups` shipped
    /// orphaned.
    #[test]
    fn every_card_is_reachable_from_the_umbrella() {
        let cat = Catalog::from_manifest_yaml(crate::MANIFEST_YAML).expect("manifest");
        let mut templates = HashMap::new();
        collect_templates(&crate::TEMPLATES, &mut templates);

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut queue = vec!["operate".to_string()];
        while let Some(id) = queue.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(entry) = cat.get(&id) else { continue };
            let Some(source) = templates.get(&entry.template_path) else {
                continue;
            };
            for captures in ["skill(", "skill_read(", "skill_opt("] {
                let mut rest = source.as_str();
                while let Some(idx) = rest.find(captures) {
                    let after = &rest[idx + captures.len()..];
                    let Some(open) = after.find('"') else { break };
                    let inner = &after[open + 1..];
                    let Some(close) = inner.find('"') else { break };
                    queue.push(inner[..close].to_string());
                    rest = &inner[close..];
                }
            }
        }

        let orphans: Vec<&str> = cat
            .iter()
            .map(|e| e.id.as_str())
            .filter(|id| !seen.contains(*id))
            .collect();
        assert!(
            orphans.is_empty(),
            "cards unreachable from the `operate` umbrella              (add a routing row or a cross-link): {orphans:?}"
        );
    }
}
