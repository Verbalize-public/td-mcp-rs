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
# execute_python payload caps — keep framed JSON well under the 32 MiB IPC
# MAX_FRAME.
SCRIPT_MAX_BYTES = 4 * 1024 * 1024
RESULT_MAX_BYTES = 4 * 1024 * 1024

# Wire method names — must match tdmcp_core::BridgeMethod::wire_str() exactly.
BRIDGE_METHODS: tuple[str, ...] = (
    "execute_python",
    "capture",
    "inspect",
    "mutate_nodes",
    "api_help",
    "editor_context",
    "palette_probe",
    "ping",
)

INSPECT_PATHS_LIMIT = 256
CHILDREN_ROSTER_LIMIT = 256

# Operator comment echo caps (inspect). `OP.comment` is an unbounded str — a
# 5000-char comment is accepted by TD — so the read surface truncates rather
# than letting a roster of 256 children blow the payload. Truncated values end
# in `_COMMENT_TRUNC_MARK`; the node's own field also sets `commentTruncated`.
COMMENT_MAX_CHARS = 1024
COMMENT_ROSTER_MAX_CHARS = 160
_COMMENT_TRUNC_MARK = "…"

# Shader lint caps (scan/consumer budgets reported via truncation markers).
SHADER_SCAN_LIMIT = 2048
SHADER_CONSUMER_LIMIT = 64
EDITOR_SELECTION_LIMIT = 256
EDITOR_PANES_LIMIT = 64
CAPTURE_VIEWER_NAME = "capture_viewer"
# Hard pre-flight reject for capture's `maxSize` (longer-side px before PNG
# encode). `maxSize: null` means native resolution with no bound otherwise —
# an unbounded PNG+base64 payload can blow the 16 MiB IPC frame and kill the
# whole bridge session, not just the one call.
CAPTURE_MAX_SIZE = 1536

# palette_probe caps. The batch cap is the load-bearing one: a probe loads
# arbitrary components, and a component that wedges TD takes its whole batch
# with it — so a batch stays small enough that the loss is cheap. Mirrored in
# Rust as palette::PROBE_BATCH_MAX.
PALETTE_PROBE_BATCH_LIMIT = 8
PALETTE_CHILD_ROSTER_LIMIT = 64
PALETTE_PAR_PAGE_LIMIT = 16
PALETTE_PARS_PER_PAGE_LIMIT = 64
PALETTE_SCRATCH_NAME = "tdmcp_probe"
# A palette wrapper's `help` DAT is the component's own documentation and the
# best raw material for a card — worth more budget than a node comment.
PALETTE_HELP_MAX_CHARS = 4000
# Thumbnail edge cap for `palette_probe thumbnails:true`. Small on purpose: the
# PNG rides the same IPC frame as the digest, and the only consumer is a GUI
# tile. Mirrored in Rust as palette::THUMB_MAX_SIZE.
PALETTE_THUMB_MAX_SIZE = 256

# api_help batch / payload caps (mirrored in Rust tools.rs).
API_HELP_QUERIES_LIMIT = 64
API_HELP_MEMBERS_SUMMARY = 128
API_HELP_MEMBERS_DETAILED = 1024
API_HELP_CLASSES_LIMIT = 2048
API_HELP_MODULE_SAMPLE = 32

CHOP_DATA_MAX_CHANNELS = 64
CHOP_DATA_MAX_SAMPLES = 1024
CHOP_DATA_MAX_SCALARS = 32768

_LOGS_RETURN_MAX = 128 * 1024
_DEBUG_DAT_RING_MAX = 256 * 1024
_TRUNC_MARK = "\n…[truncated]\n"

_BLACK_MEAN_THRESHOLD = 1.0 / 255.0
_UNIFORM_RANGE_THRESHOLD = 2.0 / 255.0

ENABLE_EXPR_EVAL_LIMIT = 64
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


