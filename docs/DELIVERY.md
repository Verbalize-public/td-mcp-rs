# Delivery

## Artifacts

| Artifact | Role |
| --- | --- |
| `tdmcp-daemon` binary | Control plane + MCP + admin API |
| `tdmcp-gui` binary | Tray + dashboard |
| `bridge/` | Python package + `manifest.json` beside install/data dir |
| `diagnostics/catalog.yaml` | Diagnostic catalog |
| bootstrap `.tox` | Tiny TD dialer (handshake → FS load) |

## Config precedence

**CLI args > env vars (`TDMCP_*`) > RC file > built-in defaults.**

Default MCP listen: `127.0.0.1:9860`. Data dir:

| OS | Path |
| --- | --- |
| Windows | `%LOCALAPPDATA%/tdmcp-rs/` |
| macOS | `~/Library/Application Support/tdmcp-rs/` |
| Linux | `$XDG_DATA_HOME/tdmcp-rs/` or `~/.local/share/tdmcp-rs/` |

## Auto-upsert — Shipped

Cursor registers `tdmcp-daemon` with `args: ["mcp"]`. On MCP connect:

1. `mcp` calls `ensure` — health check, lockfile, detached spawn if needed, poll
   until healthy.
2. Stdio MCP proxy forwards tool requests to `http://127.0.0.1:{port}/mcp/rpc`.
3. Stale lockfile (pid dead) → reclaim.

The long-lived HTTP daemon survives MCP client restarts; only the stdio proxy
process is respawned.

## Assets

Bridge, diagnostic catalog, and bootstrap `.tox` are embedded in the
`tdmcp-daemon` binary. `install`, `ensure`, `start`, and `mcp` extract them
into the data dir on first use (no separate asset bundle required for dev
builds).

## Packaging

`cargo run -p xtask -- dist` copies the release `tdmcp-daemon` binary into
`target/dist/` (bridge, catalog, and bootstrap `.tox` are embedded in the
binary). If a release `tdmcp-gui` binary already exists, it is copied too.
