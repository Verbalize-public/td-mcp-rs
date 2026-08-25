//! Typed failures for project I/O. Diagnostic-code mapping (`tdmcp.project.*`)
//! happens at the MCP layer; these variants carry the structured cause.

use std::path::PathBuf;

/// Project I/O failure.
#[derive(Debug, thiserror::Error)]
pub enum ProjectIoError {
    /// Underlying filesystem error.
    #[error("project io fs error at {path}: {source}")]
    Fs {
        /// Path acted on.
        path: PathBuf,
        /// Wrapped OS error.
        #[source]
        source: std::io::Error,
    },
    /// Requested file does not exist.
    #[error("source not found: {0}")]
    SourceNotFound(PathBuf),
    /// Packed-sniff failed: neither `.toe` nor `.tox` magic.
    #[error("not a packed TD project (bad magic): {0}")]
    NotPackedFormat(PathBuf),
    /// Official tool binary missing after full resolution.
    #[error("official tool {tool} not found; searched: {searched:?}")]
    ToolMissing {
        /// Which tool ("toeexpand" | "toecollapse").
        tool: String,
        /// Every location attempted (config/env/scan candidates).
        searched: Vec<String>,
    },
    /// Exactly one of expand/collapse was configured explicitly.
    #[error("both expand_path and collapse_path must be set together")]
    ToolPairPartial,
    /// Expand produced no usable artifacts (dir + toc absent).
    #[error("toeexpand did not produce expand artifacts next to {packed}")]
    ExpandOutputMissing {
        /// Packed input path.
        packed: PathBuf,
    },
    /// Collapse produced no usable output (file absent or empty).
    #[error("toecollapse did not produce a packed file at {out}")]
    CollapseOutputMissing {
        /// Expected output path.
        out: PathBuf,
    },
    /// Destination exists while overwrite policy is `fail`.
    #[error("destination already exists: {0}")]
    DestExists(PathBuf),
    /// Source directory does not look like an expand dir (missing .toc / .build).
    #[error("not an expand directory: {dir} ({reason})")]
    SrcNotExpandDir {
        /// Offending directory.
        dir: PathBuf,
        /// What invalidated it.
        reason: String,
    },
    /// Toc entry escapes its expand root (path traversal).
    #[error("toc entry escapes expand root: {entry}")]
    TocEscape {
        /// The offending entry text.
        entry: String,
    },
    /// `.toc` unreadable or unparsable.
    #[error("invalid toc at {path}: {reason}")]
    TocInvalid {
        /// Toc path.
        path: PathBuf,
        /// Parse failure description.
        reason: String,
    },
}
