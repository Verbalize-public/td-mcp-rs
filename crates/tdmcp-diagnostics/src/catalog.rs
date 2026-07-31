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
    #[serde(default)]
    pub mitigation: Vec<String>,
    /// References.
    #[serde(default)]
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
            references: entry.references.clone(),
            raw_traceback: None,
        })
    }
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
    }
}
