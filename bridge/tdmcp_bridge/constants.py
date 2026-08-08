"""Shared numeric / wire constants for the bridge package."""

from __future__ import annotations

__protocol_version__ = "1"
__min_daemon__ = "0.1.0"

# Idle liveness — must match tdmcp-daemon HeartbeatConfig::production.
HEARTBEAT_INTERVAL_S = 5.0
PONG_TIMEOUT_S = 8.0
IDLE_DEAD_S = 20.0
# Short poll so serve_queued can notice IDLE_DEAD without blocking forever.
_READ_POLL_S = 1.0
# Upper bound for worker wait on main-thread process_pending (handshake may lower).
# Aligns with tdmcp-mcp BRIDGE_TIMEOUT (180s) when the daemon omits maxCallWaitSecs.
DEFAULT_MAX_CALL_WAIT_S = 180.0
# execute_python payload caps — keep framed JSON well under the 16 MiB IPC MAX_FRAME.
SCRIPT_MAX_BYTES = 1 * 1024 * 1024
RESULT_MAX_BYTES = 1 * 1024 * 1024

# Wire method names — must match tdmcp_core::BridgeMethod::wire_str() exactly.
BRIDGE_METHODS: tuple[str, ...] = (
    "execute_python",
    "capture",
    "inspect",
    "mutate_nodes",
    "api_help",
    "editor_context",
    "ping",
)

INSPECT_PATHS_LIMIT = 96
CHILDREN_ROSTER_LIMIT = 96
EDITOR_SELECTION_LIMIT = 96
EDITOR_PANES_LIMIT = 32
CAPTURE_VIEWER_NAME = "capture_viewer"

# api_help batch / payload caps (mirrored in Rust tools.rs).
API_HELP_QUERIES_LIMIT = 32
API_HELP_MEMBERS_SUMMARY = 40
API_HELP_MEMBERS_DETAILED = 512
API_HELP_CLASSES_LIMIT = 1024
API_HELP_MODULE_SAMPLE = 32

CHOP_DATA_MAX_CHANNELS = 32
CHOP_DATA_MAX_SAMPLES = 256
CHOP_DATA_MAX_SCALARS = 4096

# capture pop_data caps (mirrors chop_data scalar budget).
POP_DATA_MAX_POINTS = 256
POP_DATA_MAX_ATTRS = 8
POP_DATA_MAX_SCALARS = 4096

_LOGS_RETURN_MAX = 32 * 1024
_DEBUG_DAT_RING_MAX = 64 * 1024
_TRUNC_MARK = "\n…[truncated]\n"

_BLACK_MEAN_THRESHOLD = 1.0 / 255.0
_UNIFORM_RANGE_THRESHOLD = 2.0 / 255.0

ENABLE_EXPR_EVAL_LIMIT = 32
ENABLE_PARM_WARN_MARKERS = ("enable parm expressions", "enable expression")
_ENABLE_EXPR_FAILED_CODE = "tdmcp.par.enable_expr_failed"
_ENABLE_EXPR_MITIGATION = [
    "Fix custom parameter enableExpr (Component Editor)",
    "Re-inspect after correcting the expression",
]

# Operate-relevant subset of TD OP_Class "Common Flags" (docs.derivative.ca).
# Editor/UI-only flags (current, selected, expose, showCustomOnly, showDocked,
# python) are intentionally omitted.
_FLAG_NAMES = frozenset(
    {
        "activeViewer",
        "allowCooking",
        "bypass",
        "cloneImmune",
        "display",
        "lock",
        "render",
        "viewer",
    }
)


