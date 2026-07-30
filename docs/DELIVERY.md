# Delivery

## Artifacts

| Artifact | Role |
| --- | --- |
| `tdmcp-daemon` binary | Control plane + MCP + admin API + (default) in-process tray UI |
| `bridge/` | Python package + `manifest.json` beside install/data dir |
| `diagnostics/catalog.yaml` | Diagnostic catalog |
| bootstrap `.tox` | Tiny TD dialer (handshake → FS load) |

The tray dashboard lives in the `tdmcp-gui` **library** crate, linked into
`tdmcp-daemon` when the default `gui` Cargo feature is enabled. There is no
separate `tdmcp-gui` binary.

## Config precedence

**CLI args > env vars (`TDMCP_*`) > RC file > built-in defaults.**

Default MCP listen: `127.0.0.1:9860`. Data dir:

| OS | Path |
| --- | --- |
| Windows | `%LOCALAPPDATA%/tdmcp-rs/` |
| macOS | `~/Library/Application Support/tdmcp-rs/` |
| Linux | `$XDG_DATA_HOME/tdmcp-rs/` or `~/.local/share/tdmcp-rs/` |

## GUI feature

| Build / runtime | Behavior |
| --- | --- |
| Default (`cargo build -p tdmcp-daemon`) | `gui` feature on; `start` shows tray + toast (dashboard hidden until opened) |
| `--no-default-features` | Headless binary (no egui/tray linked) |
| `--no-gui` / `TDMCP_NO_GUI=1` on `start` / `ensure` / `mcp` | Headless even when `gui` is compiled in |

## Auto-upsert — Shipped

Cursor registers `tdmcp-daemon` with `args: ["mcp"]`. On MCP connect:

1. `mcp` calls `ensure` — health check, lockfile, detached spawn if needed, poll
   until healthy.
2. Stdio MCP proxy forwards tool requests to `http://127.0.0.1:{port}/mcp/rpc`.
3. Stale lockfile (pid dead) → reclaim.
4. Detached `start` (default) brings up the in-process tray with the daemon.

The long-lived HTTP daemon survives MCP client restarts; only the stdio proxy
process is respawned.

## Singleton

One owner per port: `daemon.lock` + TCP bind. Second healthy `start` refuses.
`/admin/restart` clears the lock, spawns a replacement, then exits; the new
process retries bind for a few seconds.

## Assets

Bridge, diagnostic catalog, and bootstrap `.tox` are embedded in the
`tdmcp-daemon` binary. `install`, `ensure`, `start`, and `mcp` extract them
into the data dir on first use (no separate asset bundle required for dev
builds).

## Packaging

`cargo run -p xtask -- dist` always rebuilds `tdmcp-daemon` with
`--features gui`, then copies it into `target/dist/` (bridge, catalog, and
bootstrap `.tox` are embedded in the binary). This avoids shipping a stale
headless binary from a prior `--no-default-features` build.
