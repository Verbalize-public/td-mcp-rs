//! Catalog loader and baked-in fallback.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::envelope::{
    DiagnosticItem, DiagnosticLayer, DiagnosticSeverity, DiagnosticSpan, Reference,
};

/// Errors loading or looking up the diagnostic catalog.
#[derive(Debug, Error)]
pub enum CatalogError {
    /// I/O failure reading the YAML file.
    #[error("failed to read catalog: {0}")]
    Io(#[from] std::io::Error),
    /// YAML parse failure.
    #[error("failed to parse catalog YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),
    /// Code missing from catalog.
    #[error("unknown diagnostic code: {0}")]
    UnknownCode(String),
}

/// One catalog entry (source of truth for mitigation / references).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    /// Stable code.
    pub code: String,
    /// Default layer.
    pub layer: DiagnosticLayer,
    /// Message template (may contain `{op_path}` style placeholders later).
    pub message: String,
    /// Mitigation steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mitigation: Vec<String>,
    /// References.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<Reference>,
}

#[derive(Debug, Deserialize)]
struct CatalogFile {
    entries: Vec<CatalogEntry>,
}

/// Loaded diagnostic catalog.
#[derive(Debug, Clone)]
pub struct Catalog {
    entries: HashMap<String, CatalogEntry>,
}

impl Catalog {
    /// Load from a YAML file on disk.
    pub fn load_path(path: &Path) -> Result<Self, CatalogError> {
        let text = std::fs::read_to_string(path)?;
        Self::from_yaml(&text)
    }

    /// Parse YAML text.
    pub fn from_yaml(text: &str) -> Result<Self, CatalogError> {
        let file: CatalogFile = serde_yaml::from_str(text)?;
        let mut entries = HashMap::new();
        for entry in file.entries {
            entries.insert(entry.code.clone(), entry);
        }
        Ok(Self { entries })
    }

    /// Baked-in minimal catalog used when the file is missing.
    #[must_use]
    pub fn fallback() -> Self {
        const YAML: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../diagnostics/catalog.yaml"
        ));
        Self::from_yaml(YAML).unwrap_or_else(|_| Self {
            entries: HashMap::new(),
        })
    }

    /// Lookup by code.
    pub fn get(&self, code: &str) -> Result<&CatalogEntry, CatalogError> {
        self.entries
            .get(code)
            .ok_or_else(|| CatalogError::UnknownCode(code.to_owned()))
    }

    /// Whether the code is known.
    #[must_use]
    pub fn contains(&self, code: &str) -> bool {
        self.entries.contains_key(code)
    }

    /// All known codes (for completeness tests).
    #[must_use]
    pub fn codes(&self) -> Vec<&str> {
        let mut codes: Vec<&str> = self.entries.keys().map(String::as_str).collect();
        codes.sort_unstable();
        codes
    }

    /// Build an error item from a catalog entry + span/message override.
    pub fn build_error(
        &self,
        code: &str,
        span: DiagnosticSpan,
        message_override: Option<String>,
    ) -> Result<DiagnosticItem, CatalogError> {
        let entry = self.get(code)?;
        Ok(DiagnosticItem {
            severity: DiagnosticSeverity::Error,
            code: entry.code.clone(),
            layer: entry.layer,
            message: message_override.unwrap_or_else(|| entry.message.clone()),
            span,
            context: Default::default(),
            lints: Vec::new(),
            mitigation: entry.mitigation.clone(),
            references: normalize_references(entry.references.clone()),
            raw_traceback: None,
            exception: None,
        })
    }
}

/// Ensure `kind:doc` references carry `uri: tdmcp://docs/<id>` when id is set.
fn normalize_references(mut refs: Vec<Reference>) -> Vec<Reference> {
    for r in &mut refs {
        if r.kind == "doc" && r.uri.is_none() {
            if let Some(id) = r.id.as_deref() {
                if !id.is_empty() {
                    r.uri = Some(format!("tdmcp://docs/{id}"));
                }
            }
        }
    }
    refs
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn fallback_catalog_loads_starter_codes() {
        let cat = Catalog::fallback();
        assert!(cat.contains("tdmcp.bridge.unknown_pid"));
        assert!(cat.contains("tdmcp.op.not_found"));
        assert!(cat.contains("tdmcp.perception.black_frame"));
        assert!(cat.contains("tdmcp.perception.uniform_frame"));
        assert!(cat.contains("tdmcp.script.execution_failed"));
    }

    #[test]
    fn build_error_uses_mitigation() {
        let cat = Catalog::fallback();
        let item = cat
            .build_error(
                "tdmcp.bridge.queue_busy",
                DiagnosticSpan {
                    tool: "execute_python".into(),
                    mutation_index: None,
                    field: None,
                    line: None,
                    column: None,
                    snippet: None,
                },
                None,
            )
            .expect("known code");
        assert!(!item.mitigation.is_empty());
        assert_eq!(item.layer, DiagnosticLayer::Fleet);
        let doc = item
            .references
            .iter()
            .find(|r| r.kind == "doc")
            .expect("doc ref");
        assert_eq!(doc.id.as_deref(), Some("tooling-concurrency"));
        assert_eq!(doc.uri.as_deref(), Some("tdmcp://docs/tooling-concurrency"));
    }

    #[test]
    fn catalog_doc_ids_match_skills_manifest() {
        let cat = Catalog::fallback();
        let manifest = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../skills/MANIFEST.yaml"
        ));
        let skill_ids = parse_manifest_skill_ids(manifest);
        const KNOWN_TOOLS: &[&str] = &[
            "fleet",
            "execute_python",
            "inspect",
            "mutate_nodes",
            "capture",
            "api_help",
            "editor_context",
            "describe_tools",
            "td_installs",
        ];
        for code in cat.codes() {
            let entry = cat.get(code).expect("code");
            for r in &entry.references {
                let kind = r.kind.as_str();
                assert!(
                    matches!(kind, "doc" | "tool" | "api_help"),
                    "{code}: unknown reference kind `{kind}` (expected doc|tool|api_help)"
                );
                if kind == "doc" {
                    let id = r.id.as_deref().unwrap_or("");
                    assert!(!id.is_empty(), "{code}: kind:doc missing id");
                    assert!(
                        skill_ids.contains(id),
                        "{code}: kind:doc id `{id}` not in skills/MANIFEST.yaml"
                    );
                    let expected = format!("tdmcp://docs/{id}");
                    let uri = r.uri.as_deref().unwrap_or("");
                    assert!(
                        uri.is_empty() || uri == expected,
                        "{code}: kind:doc uri `{uri}` must be empty or `{expected}`"
                    );
                } else if kind == "tool" {
                    let id = r.id.as_deref().unwrap_or("");
                    assert!(
                        KNOWN_TOOLS.contains(&id),
                        "{code}: kind:tool id `{id}` is not a shipped MCP tool"
                    );
                }
            }
        }
    }

    fn parse_manifest_skill_ids(manifest: &str) -> std::collections::HashSet<String> {
        // MANIFEST keys are top-level `id:` lines (no leading spaces).
        let mut ids = std::collections::HashSet::new();
        for line in manifest.lines() {
            if line.is_empty()
                || line.starts_with('#')
                || line.starts_with(' ')
                || line.starts_with('\t')
            {
                continue;
            }
            if let Some(key) = line.strip_suffix(':') {
                if !key.is_empty() && !key.contains(' ') {
                    ids.insert(key.to_owned());
                }
            }
        }
        ids
    }
}
