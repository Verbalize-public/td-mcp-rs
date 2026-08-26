//! Embedded operate docs as MCP resources (`tdmcp://docs/*`).
//!
//! The [`ResourceProvider`] wraps the [`TemplateEngine`](super::template) to
//! serve skill cards via MCP `resources/list` and `resources/read`.  Templates
//! are rendered in [`RenderMode::Mcp`](super::template::RenderMode) so that
//! every `{{ skill("id") }}` call produces a `tdmcp://docs/<id>` URI.

use rmcp::model::{
    ListResourcesResult, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
};

use crate::template::TemplateEngine;

const URI_PREFIX: &str = "tdmcp://docs/";

/// Short MCP `instructions` — when → which resource URI.
pub const SERVER_INSTRUCTIONS: &str = "\
td-mcp-rs control plane for TouchDesigner. \
Call `fleet` to pick a connected `pid`, then bridged tools one-at-a-time \
(`inspect` / `mutate_nodes` / `capture` / `execute_python` / `api_help` / `editor_context`). \
Never parallel bridged calls on the same pid (see tdmcp://docs/tooling-concurrency). \
Prefer `inspect` over Python for networks. \
Before OpSketch / multi-node mutate: resources/read tdmcp://docs/opsketch-notation. \
Before execute_python or expressions: resources/read tdmcp://docs/python-api. \
DoD / look: tdmcp://docs/definition-of-done , tdmcp://docs/look-grade. \
Paused / stale: tdmcp://docs/play-state. \
No connected pid? `spawn_td` / `kill_td` — tdmcp://docs/lifecycle. \
Calls stalling or a modal popup? `dialogs` — tdmcp://docs/popups. \
Offline .toe/.tox, installs, bridge install: `td_installs` / `project_unpack` / \
`project_pack` / `project_lint` / `project_install_bridge` — tdmcp://docs/project-io. \
Operate umbrella: tdmcp://docs/operate. \
List all cards via resources/list.";

/// Stdio-proxy variant (notes reconnect; resources served locally).
pub const STDIO_SERVER_INSTRUCTIONS: &str = "\
td-mcp-rs control plane (stdio proxy). \
Call `fleet` to pick a connected `pid`, then bridged tools one-at-a-time \
(`inspect` / `mutate_nodes` / `capture` / `execute_python` / `api_help` / `editor_context`). \
Never parallel bridged calls on the same pid (tdmcp://docs/tooling-concurrency). \
Prefer `inspect` over Python walks. \
Before OpSketch: resources/read tdmcp://docs/opsketch-notation. \
Before Python: resources/read tdmcp://docs/python-api. \
No connected pid? `spawn_td` / `kill_td` — tdmcp://docs/lifecycle. \
Calls stalling or a modal popup? `dialogs` — tdmcp://docs/popups. \
Offline .toe/.tox, installs, bridge install: tdmcp://docs/project-io. \
Operate pack: tdmcp://docs/operate ; full catalog: resources/list. \
Tool calls forward to the HTTP daemon; operate docs are served locally from the embed. \
If the daemon restarts, the proxy reconnects (never auto-spawns) and returns \
tdmcp.daemon.unreachable for the failed call.";

/// Server capabilities: tools + resources.
#[must_use]
pub fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities::builder()
        .enable_tools()
        .enable_resources()
        .build()
}

fn uri_for(id: &str) -> String {
    format!("{URI_PREFIX}{id}")
}

// ---------------------------------------------------------------------------
// Resource provider
// ---------------------------------------------------------------------------

/// Serves MCP resource list/read from the template engine (always MCP mode).
pub struct ResourceProvider {
    engine: TemplateEngine,
}

impl ResourceProvider {
    /// Build from the embedded `MANIFEST.yaml` + `templates/` tree.
    pub fn from_embedded() -> Result<Self, String> {
        let catalog = crate::template::Catalog::from_manifest_yaml(crate::MANIFEST_YAML)?;
        let engine = crate::template::TemplateEngine::new(catalog, &crate::TEMPLATES)?;
        Ok(Self::new(engine))
    }

    /// Wrap a ready template engine.
    pub fn new(engine: TemplateEngine) -> Self {
        Self { engine }
    }

    /// List all operate resources.
    #[must_use]
    pub fn list_resources(&self) -> ListResourcesResult {
        let resources: Vec<Resource> = self
            .engine
            .catalog()
            .iter()
            .map(|entry| {
                Resource::new(uri_for(&entry.id), entry.id.clone())
                    .with_description(format!("{} — {}", entry.title, entry.description))
                    .with_mime_type("text/markdown")
                // We don't know the size before rendering; MCP clients cope.
            })
            .collect();
        ListResourcesResult::with_all_items(resources)
    }

    /// Read one resource by URI (`tdmcp://docs/<id>`).
    pub fn read_resource(&self, uri: &str) -> Result<ReadResourceResult, String> {
        let id = uri.strip_prefix(URI_PREFIX).unwrap_or(uri);

        let body = self.engine.render(id, crate::template::RenderMode::Mcp)?;
        let contents = ResourceContents::text(body, uri_for(id)).with_mime_type("text/markdown");
        Ok(ReadResourceResult::new(vec![contents]))
    }

    /// Number of catalog entries.
    pub fn catalog_len(&self) -> usize {
        self.engine.catalog().len()
    }

    /// Access the underlying engine (for FS-mode rendering, install, etc.).
    pub fn engine(&self) -> &TemplateEngine {
        &self.engine
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use crate::template::Catalog;

    fn test_provider() -> ResourceProvider {
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
tooling-concurrency:
  template_path: touchdesigner/reference/tooling-concurrency.jinja.md
  output_path: touchdesigner/reference/tooling-concurrency.md
  title: Tooling concurrency
  description: Sequential bridged tools
play-state:
  template_path: touchdesigner/reference/play-state.jinja.md
  output_path: touchdesigner/reference/play-state.md
  title: Play state
  description: Paused timeline
"#;
        let catalog = Catalog::from_manifest_yaml(yaml).expect("catalog");

        // Build an engine with empty templates — the content is just raw
        // Markdown for resource-test purposes. No Jinja calls needed.
        let engine = {
            use std::collections::HashMap;
            let mut templates = HashMap::new();
            for entry in catalog.iter() {
                templates.insert(
                    entry.template_path.clone(),
                    format!("# {}\n\n{}", entry.id, entry.description),
                );
            }

            let mut env = minijinja::Environment::new();
            env.set_auto_escape_callback(|_| minijinja::AutoEscape::None);
            // No skill/skill_read functions needed — test content has no
            // Jinja calls.

            crate::template::TemplateEngine::new_for_test(catalog, env, templates)
        };

        ResourceProvider::new(engine)
    }

    #[test]
    fn catalog_sufficient() {
        let provider = test_provider();
        assert!(provider.catalog_len() >= 4);
    }

    #[test]
    fn list_resources_matches_catalog() {
        let provider = test_provider();
        let list = provider.list_resources();
        assert_eq!(list.resources.len(), provider.catalog_len());
        // Each resource should have a tdmcp:// URI.
        for r in &list.resources {
            assert!(r.uri.starts_with("tdmcp://docs/"));
        }
    }

    #[test]
    fn read_resource_returns_body() {
        let provider = test_provider();
        let result = provider
            .read_resource("tdmcp://docs/operate")
            .expect("operate");
        assert!(!result.contents.is_empty());
    }

    #[test]
    fn unknown_resource_errors() {
        let provider = test_provider();
        let result = provider.read_resource("tdmcp://docs/nope");
        assert!(result.is_err());
    }
}
