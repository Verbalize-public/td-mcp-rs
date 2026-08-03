# Delivery

## Artifacts

| Artifact | Role |
| --- | --- |
| `tdmcp-daemon` binary | Control plane + MCP + admin API + (default) in-process tray UI |
| `bridge/` | Python package + `manifest.json` beside install/data dir |
| `diagnostics/catalog.yaml` | Diagnostic catalog |
| bootstrap `.tox` | Tiny TD dialer COMP `tdmcp_rs` (handshake → FS load of `bridge/`). Embedded in the daemon; extracted to `{dataDir}/bootstrap.tox`. Rebuild recipe: [`scripts/pack_bootstrap_tox.md`](../scripts/pack_bootstrap_tox.md) |

The tray dashboard lives in the `tdmcp-gui` **library** crate, linked into
`tdmcp-daemon` when the default `gui` Cargo feature is enabled. There is no
separate `tdmcp-gui` binary.

## Config

Source of truth: TOML file owned by crate `tdmcp-config` (see
[`docs/CONFIG.md`](CONFIG.md)).

**CLI args / env (`TDMCP_*`) > config.toml > built-in defaults.**

| Kind | Default path |
| --- | --- |
| Config file | `%APPDATA%/tdmcp-rs/config.toml` (Windows); Application Support / XDG config elsewhere |
| Data dir | `%LOCALAPPDATA%/tdmcp-rs/` (Windows); Application Support / XDG data elsewhere |

Notable `[daemon]` fields: `keep_alive`, `always_on`, `show_tray`.
`install` always resets `config.toml` to the embedded template; `start` /
`ensure` / `mcp` only create-if-missing.

## GUI feature

| Build / runtime | Behavior |
| --- | --- |
| Default (`cargo build -p tdmcp-daemon`) | `gui` feature on; `start` shows tray + toast (dashboard hidden until opened); gear opens Settings |
| `--no-default-features` | Headless binary (no egui/tray linked) |
| `--no-gui` / `TDMCP_NO_GUI=1` / `show_tray = false` | Headless even when `gui` is compiled in |

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

`cargo run -p xtask -- dist` first runs [`scripts/kill-daemons`](../scripts/kill-daemons.ps1)
(soft `/admin/shutdown`, then force-kills only workspace `target/release` /
`target/dist` images) so leftover Cursor `mcp` shims do not lock the exe.
It then rebuilds `tdmcp-daemon` with `--features gui` and copies it into
`target/dist/` (bridge, catalog, and bootstrap `.tox` are embedded in the
binary). This avoids shipping a stale headless binary from a prior
`--no-default-features` build.

If Cursor still has the MCP server connected, it may respawn `tdmcp-daemon mcp`
and re-lock the file — pause/reload MCP when rebuild still fails after kill.
