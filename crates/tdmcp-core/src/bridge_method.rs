//! Bridge IPC method enum — one wire concept for method + queue label.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Wire method names spoken on the daemon↔bridge IPC pipe.
///
/// Queue display labels hang off the same enum via [`Self::queue_label`] —
/// never two free strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeMethod {
    /// Run Python in TD.
    ExecutePython,
    /// Perception capture.
    Capture,
    /// Structural inspect.
    Inspect,
    /// Ordered create / set / delete.
    MutateNodes,
    /// Live TD Python API cards / class index.
    ApiHelp,
    /// Live editor pane / selection snapshot.
    EditorContext,
    /// Liveness ping.
    Ping,
}

impl BridgeMethod {
    /// Wire string matching Python `HANDLERS` keys exactly.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::ExecutePython => "execute_python",
            Self::Capture => "capture",
            Self::Inspect => "inspect",
            Self::MutateNodes => "mutate_nodes",
            Self::ApiHelp => "api_help",
            Self::EditorContext => "editor_context",
            Self::Ping => "ping",
        }
    }

    /// Task-queue display label (may differ from wire method).
    #[must_use]
    pub const fn queue_label(self) -> &'static str {
        match self {
            Self::ExecutePython => "PythonEval",
            Self::Capture => "Capture",
            Self::Inspect => "Inspect",
            Self::MutateNodes => "Mutate",
            Self::ApiHelp => "ApiHelp",
            Self::EditorContext => "EditorContext",
            Self::Ping => "Ping",
        }
    }

    /// All known methods (for parity tests).
    pub const ALL: &[Self] = &[
        Self::ExecutePython,
        Self::Capture,
        Self::Inspect,
        Self::MutateNodes,
        Self::ApiHelp,
        Self::EditorContext,
        Self::Ping,
    ];

    /// Parse a wire method string.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "execute_python" => Some(Self::ExecutePython),
            "capture" => Some(Self::Capture),
            "inspect" => Some(Self::Inspect),
            "mutate_nodes" => Some(Self::MutateNodes),
            "api_help" => Some(Self::ApiHelp),
            "editor_context" => Some(Self::EditorContext),
            "ping" => Some(Self::Ping),
            _ => None,
        }
    }
}

impl fmt::Display for BridgeMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.wire_str())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn wire_roundtrip() {
        for m in BridgeMethod::ALL {
            assert_eq!(BridgeMethod::from_wire(m.wire_str()), Some(*m));
        }
    }

    #[test]
    fn queue_labels_are_stable() {
        assert_eq!(BridgeMethod::ExecutePython.queue_label(), "PythonEval");
        assert_eq!(BridgeMethod::Capture.queue_label(), "Capture");
    }
}
