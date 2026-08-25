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
}

impl TemplateEngine {
    /// Build the engine from a parsed [`Catalog`] and the embedded templates
    /// directory (`skills/templates/`).
    pub fn new(catalog: Catalog, templates_dir: &'static Dir<'_>) -> Result<Self, String> {
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
        let env = build_env(Arc::clone(&catalog));

        Ok(Self {
            catalog,
            env,
            templates,
        })
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
        }
    }

    /// Render one template by its catalog id in the given mode.
    pub fn render(&self, id: &str, mode: RenderMode) -> Result<String, String> {
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

    /// Render all catalog entries in the given mode.
    ///
    /// Returns a `Vec` of `(output_path, content)` pairs.
    pub fn render_all(&self, mode: RenderMode) -> Result<Vec<(String, String)>, String> {
        let mut results = Vec::with_capacity(self.catalog.len());
        for entry in self.catalog.iter() {
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
fn build_env(catalog: Arc<Catalog>) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| minijinja::AutoEscape::None);

    let cat = Arc::clone(&catalog);
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

    fn engine_for(cat: Catalog, source: &str) -> TemplateEngine {
        let mut templates = HashMap::new();
        for entry in cat.iter() {
            templates.insert(entry.template_path.clone(), source.to_string());
        }

        let cat = Arc::new(cat);
        let env = build_env(Arc::clone(&cat));

        TemplateEngine {
            catalog: cat,
            env,
            templates,
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
            // Every referenced id must resolve.
            for captures in ["skill(", "skill_read("] {
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
}
