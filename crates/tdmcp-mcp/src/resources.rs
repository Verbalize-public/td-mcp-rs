//! Embedded operate docs as MCP resources (`tdmcp://docs/*`).
//!
//! Source of truth: repo `skills/touchdesigner/**` (also extracted to
//! `{dataDir}/skills/` by the daemon install path).

use include_dir::{include_dir, Dir, File};
use rmcp::model::{
    ListResourcesResult, ReadResourceResult, Resource, ResourceContents, ServerCapabilities,
};

/// Compile-time embed of the skills tree (repo root `skills/`).
static SKILLS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../skills");

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

/// Catalog entry: URI id → relative path under `skills/`.
#[derive(Debug, Clone, Copy)]
struct DocEntry {
    id: &'static str,
    rel_path: &'static str,
    title: &'static str,
    description: &'static str,
}

/// Every shipped operate card (plan catalog A–C).
const CATALOG: &[DocEntry] = &[
    DocEntry {
        id: "operate",
        rel_path: "touchdesigner/SKILL.md",
        title: "TouchDesigner operate",
        description: "Umbrella skill: tool routing, hard rules, resource index",
    },
    DocEntry {
        id: "opsketch-notation",
        rel_path: "touchdesigner/reference/opsketch-notation.md",
        title: "OpSketch notation",
        description: "Primary network sketch grammar",
    },
    DocEntry {
        id: "opsketch-importance-gating",
        rel_path: "touchdesigner/reference/opsketch-importance-gating.md",
        title: "OpSketch importance gating",
        description: "Which parameters to surface in OpSketch",
    },
    DocEntry {
        id: "opsketch-examples",
        rel_path: "touchdesigner/reference/opsketch-examples.md",
        title: "OpSketch examples",
        description: "Worked OpSketch transcriptions",
    },
    DocEntry {
        id: "python-api",
        rel_path: "touchdesigner/reference/python-api.md",
        title: "TD Python API cheatsheet",
        description: "Mandatory before execute_python / expressions",
    },
    DocEntry {
        id: "custom-parameters",
        rel_path: "touchdesigner/reference/custom-parameters.md",
        title: "Custom parameters",
        description: "Custom par naming and create/update API",
    },
    DocEntry {
        id: "mutation-zones",
        rel_path: "touchdesigner/reference/mutation-zones.md",
        title: "Mutation zones",
        description: "Where agents may mutate",
    },
    DocEntry {
        id: "network-design",
        rel_path: "touchdesigner/reference/network-design.md",
        title: "Network design",
        description: "Naming, relative refs, layout hygiene",
    },
    DocEntry {
        id: "component-checklist",
        rel_path: "touchdesigner/reference/component-checklist.md",
        title: "Component checklist",
        description: "In/Out, About page, reuse audit",
    },
    DocEntry {
        id: "operator-families",
        rel_path: "touchdesigner/reference/operator-families.md",
        title: "Operator families",
        description: "Family meaning and cook-model depth",
    },
    DocEntry {
        id: "pops",
        rel_path: "touchdesigner/reference/pops.md",
        title: "POP family",
        description: "POP deep-dive",
    },
    DocEntry {
        id: "glsl",
        rel_path: "touchdesigner/reference/glsl.md",
        title: "TD GLSL dialect",
        description: "GLSL TOP/MAT dialect and workflow",
    },
    DocEntry {
        id: "shadertoy-conversion",
        rel_path: "touchdesigner/reference/shadertoy-conversion.md",
        title: "Shadertoy conversion",
        description: "Wrap-don't-rewrite Shadertoy → TD",
    },
    DocEntry {
        id: "td-glsl-ground-truth",
        rel_path: "touchdesigner/reference/td-glsl-ground-truth.md",
        title: "TD GLSL ground truth",
        description: "Uniform and resolution traps",
    },
    DocEntry {
        id: "definition-of-done",
        rel_path: "touchdesigner/reference/definition-of-done.md",
        title: "Definition of Done",
        description: "Structural PASS/FAIL/BLOCKED/SKIP",
    },
    DocEntry {
        id: "look-grade",
        rel_path: "touchdesigner/reference/look-grade.md",
        title: "Look / FPS grade",
        description: "Capture and look claim grading",
    },
    DocEntry {
        id: "tooling-concurrency",
        rel_path: "touchdesigner/reference/tooling-concurrency.md",
        title: "Tooling concurrency",
        description: "Sequential bridged tools / session_busy",
    },
    DocEntry {
        id: "play-state",
        rel_path: "touchdesigner/reference/play-state.md",
        title: "Play state",
        description: "Paused timeline and stale capture",
    },
    DocEntry {
        id: "primer/cook-and-families",
        rel_path: "touchdesigner/primer/cook-and-families.md",
        title: "Primer: cook and families",
        description: "Pull cook model and seven families",
    },
    DocEntry {
        id: "primer/editor-and-layout",
        rel_path: "touchdesigner/primer/editor-and-layout.md",
        title: "Primer: editor and layout",
        description: "Network editor and layout hygiene",
    },
    DocEntry {
        id: "primer/parameters-and-channels",
        rel_path: "touchdesigner/primer/parameters-and-channels.md",
        title: "Primer: parameters and channels",
        description: "Parameter modes and driving values",
    },
    DocEntry {
        id: "primer/scripting-surfaces",
        rel_path: "touchdesigner/primer/scripting-surfaces.md",
        title: "Primer: scripting surfaces",
        description: "Textport, Execute DATs, extensions vs MCP",
    },
    DocEntry {
        id: "primer/tox-toe-components",
        rel_path: "touchdesigner/primer/tox-toe-components.md",
        title: "Primer: tox / toe / components",
        description: "Project files and reusable COMPs",
    },
    DocEntry {
        id: "primer/glsl-and-render",
        rel_path: "touchdesigner/primer/glsl-and-render.md",
        title: "Primer: GLSL and render",
        description: "GLSL TOP/MAT and Render TOP chain",
    },
    DocEntry {
        id: "primer/performance",
        rel_path: "touchdesigner/primer/performance.md",
        title: "Primer: performance",
        description: "Cook cost and FPS evidence habits",
    },
];

fn uri_for(id: &str) -> String {
    format!("{URI_PREFIX}{id}")
}

fn find_entry(uri: &str) -> Option<&'static DocEntry> {
    let id = uri.strip_prefix(URI_PREFIX).unwrap_or(uri);
    CATALOG.iter().find(|e| e.id == id)
}

fn file_for(entry: &DocEntry) -> Option<&'static File<'static>> {
    SKILLS.get_file(entry.rel_path)
}

/// List all operate resources.
#[must_use]
pub fn list_resources() -> ListResourcesResult {
    let resources: Vec<Resource> = CATALOG
        .iter()
        .map(|e| {
            // Keep the wire shape conservative for older MCP clients (e.g. Cursor):
            // uri + name + description + mimeType + size. Avoid `title` / icons.
            let mut r = Resource::new(uri_for(e.id), e.id)
                .with_description(format!("{} — {}", e.title, e.description))
                .with_mime_type("text/markdown");
            if let Some(file) = file_for(e) {
                r = r.with_size(file.contents().len() as u64);
            }
            r
        })
        .collect();
    ListResourcesResult::with_all_items(resources)
}

/// Read one resource by URI (`tdmcp://docs/<id>`).
pub fn read_resource(uri: &str) -> Result<ReadResourceResult, String> {
    let entry = find_entry(uri).ok_or_else(|| format!("unknown resource uri: {uri}"))?;
    let file = file_for(entry).ok_or_else(|| {
        format!(
            "embedded file missing for {}: {}",
            entry.id, entry.rel_path
        )
    })?;
    let text = std::str::from_utf8(file.contents())
        .map_err(|e| format!("resource {} is not utf-8: {e}", entry.id))?;
    let contents = ResourceContents::text(text, uri_for(entry.id)).with_mime_type("text/markdown");
    Ok(ReadResourceResult::new(vec![contents]))
}

/// Number of catalog entries (tests).
#[must_use]
pub fn catalog_len() -> usize {
    CATALOG.len()
}

/// Whether every catalog path exists in the embed (tests / install sanity).
#[must_use]
pub fn catalog_files_present() -> bool {
    CATALOG.iter().all(|e| file_for(e).is_some())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn catalog_embedded_and_readable() {
        assert!(catalog_files_present());
        assert!(catalog_len() >= 25);
        let list = list_resources();
        assert_eq!(list.resources.len(), catalog_len());
        let opsketch = read_resource("tdmcp://docs/opsketch-notation").expect("opsketch");
        assert!(!opsketch.contents.is_empty());
        let operate = read_resource("tdmcp://docs/operate").expect("operate");
        assert!(!operate.contents.is_empty());
        let concurrency = read_resource("tdmcp://docs/tooling-concurrency").expect("concurrency");
        assert!(!concurrency.contents.is_empty());
        let play = read_resource("tdmcp://docs/play-state").expect("play");
        assert!(!play.contents.is_empty());
        let primer = read_resource("tdmcp://docs/primer/cook-and-families").expect("primer");
        assert!(!primer.contents.is_empty());
        assert!(read_resource("tdmcp://docs/nope").is_err());
    }
}
