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

## Auto-upsert

Cursor plugin registers `http://127.0.0.1:9860/mcp/rpc`. On connection refused,
spawn `tdmcp-daemon start` and retry. Stale lockfile (pid dead) → reclaim.

## Packaging (P2)

`cargo xtask dist` assembles release tree: binaries + bridge + catalog + tox.
Until then: `cargo build --release -p tdmcp-daemon -p tdmcp-gui`.
