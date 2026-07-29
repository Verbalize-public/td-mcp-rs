//! Diagnostic envelope and catalog for td-mcp-rs.
//!
//! Free-string-only MCP failures are forbidden — every failure maps to a
//! stable `tdmcp.*` code from [`catalog.yaml`](../../../diagnostics/catalog.yaml).

#![warn(missing_docs)]

mod catalog;
mod envelope;

pub use catalog::{Catalog, CatalogEntry, CatalogError};
pub use envelope::{
    DiagnosticContext, DiagnosticItem, DiagnosticLayer, DiagnosticLevel, DiagnosticSeverity,
    DiagnosticSpan, Diagnostics, LintItem, Reference, Suggestion,
};
