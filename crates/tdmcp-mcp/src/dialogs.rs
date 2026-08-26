//! Process-wide dialogs shared state (`tdmcp.dialogs` slot).
//!
//! Installed once by the daemon at startup (`install`), read from dispatch
//! arms via `get()` — same pattern as `init_bridge_timeouts`. `None` (tests,
//! feature off) degrades every consumer to "no data", never an error.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tdmcp_core::{DialogSnapshot, DialogSource};

/// Source + latest snapshots keyed by pid. Snapshots are refreshed by the
/// daemon watcher each `[dialogs].poll_ms`; consumers only read.
pub struct DialogsShared {
    /// Platform backend (Win32 on Windows, Null elsewhere).
    pub source: Arc<dyn DialogSource>,
    /// Latest snapshot per registered pid.
    pub snapshots: Mutex<HashMap<u32, DialogSnapshot>>,
}

static DIALOGS: OnceLock<Arc<DialogsShared>> = OnceLock::new();

/// Install the process-wide shared state. Returns false when already set
/// (first install wins — daemon startup is single-shot).
pub fn install(shared: Arc<DialogsShared>) -> bool {
    DIALOGS.set(shared).is_ok()
}

/// Process-wide access, when installed.
#[must_use]
pub fn get() -> Option<&'static Arc<DialogsShared>> {
    DIALOGS.get()
}
