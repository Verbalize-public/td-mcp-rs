# td-mcp-rs

This is not another cheap MCP for touchdesigner.
In this repo wou will find an curated MCP that focus on the following:

- Support multiple touchdesigner window
- Support multiple MCP consumer (eg: cursor + claude can coexists)
- Strong IPC allowing reliable and resilient MCP<>DAEMON<>TD networking
- Can orchestrate multiple machine (master/slave system, control two machine as one) (WIP - Architecture ready)
- Offline toe/tox edition (eg offline injection of the MCP bridge)
- Open/close touchdesigner window (WIP - POC ready, need clean implementaiton)
- Dialog detection/auto-approval (WIP, POC read, need clean implementation)
- Compatible with MAC OS and windows

## Install

Build from source (release packaging lands later):

```text
cargo build --release -p tdmcp-daemon
```

Binary: `target/release/tdmcp-daemon` (includes the tray dashboard by default
via the `gui` Cargo feature). Headless build:
`cargo build --release -p tdmcp-daemon --no-default-features`.
Bridge package and diagnostic catalog ship beside the repo (`bridge/`,
`diagnostics/catalog.yaml`).

## Quickstart

1. Build the daemon:

   ```text
   cargo build --release -p tdmcp-daemon
   ```

2. Add to Cursor `mcp.json` (see [`mcp.tdmcp.example.json`](mcp.tdmcp.example.json);
   use an absolute path; on Unix omit `.exe`):

   ```json
   {
     "mcpServers": {
       "tdmcp-rs": {
         "command": "<path>/tdmcp-daemon",
         "args": ["mcp"]
       }
     }
   }
   ```

   Keep `mcp.json` minimal — settings live in the TOML config file, not in
   MCP `args` / `env`. See [`docs/CONFIG.md`](docs/CONFIG.md).

   Cursor spawns `tdmcp-daemon mcp`, which runs `ensure` (health → lock →
   detached spawn → poll), then serves a stdio MCP proxy to
   `http://127.0.0.1:9860/mcp/rpc` (port from config). The HTTP daemon stays
   up across MCP client restarts. By default it exits after **~30s** with no
   MCP sessions and no TD bridges; set `keep_alive = true` in the config (or
   tray Settings) to disable that. The detached daemon shows a system-tray
   icon and a startup toast (dashboard hidden until you left-click the tray;
   right-click has Restart / Stop; gear opens Settings). **Stop** ends the
   process. Use `--no-gui` / `TDMCP_NO_GUI=1` or `show_tray = false` for
   headless.

   First run creates the config file (if missing) and extracts embedded
   bridge, catalog, and bootstrap `.tox` into the data dir. Config default:
   `%APPDATA%/tdmcp-rs/config.toml` (Windows). Data dir default:
   `%LOCALAPPDATA%/tdmcp-rs/` (Windows), Application Support (macOS), or XDG
   (Linux). `tdmcp-daemon install` resets the config to shipped defaults.

3. Drop the extracted bootstrap tox into a TouchDesigner project
   (`%LOCALAPPDATA%/tdmcp-rs/bootstrap.tox` on Windows — thin dialer COMP
   `tdmcp_rs`). TD dials the local IPC endpoint, handshakes, and loads the
   bridge package from the path the daemon returns. The COMP Operator Viewer
   shows a color-banded status face + live task list; Bridge page pars are
   `Connect`, `Autoconnect`, `Status`, and `Cancelqueued`. For debug only, you
   can still paste `bridge/bootstrap.py` into a Text DAT (see
   `bridge/tox_callbacks.py` for the Execute DAT pump / reconnect).
   Regenerate the embedded tox via [`scripts/pack_bootstrap_tox.md`](scripts/pack_bootstrap_tox.md).

### Power users

Manual daemon + direct Streamable HTTP (no stdio shim):

```text
tdmcp-daemon start --port 9860
# Headless: tdmcp-daemon start --port 9860 --no-gui
# MCP client URL: http://127.0.0.1:9860/mcp/rpc
# Health: GET http://127.0.0.1:9860/mcp/health → {"ok":true}
```

Tray (default): icon + toast on start; left-click toggles the compact dashboard
(Docker-style; auto-hides on focus loss). Header: gear (Settings) · `.tox` ·
Restart · Stop. Right-click: Restart / Stop. Closing the window only hides it —
use **Stop** / `tdmcp-daemon stop` to end the process. Settings edits write
`config.toml` and apply after the next restart.

Other CLI helpers: `tdmcp-daemon ensure` (spawn if down; `--force` re-extracts
embedded assets), `install` (extract assets **and** reset config to defaults;
`--force` same-version asset refresh), `status`, `stop`.

## Tools (P0)

| Tool | Job |
| --- | --- |
| `fleet` | List TD processes by `pid`, bridge status, tasks, resurrection traces |
| `execute_python` | Run Python in TD (`result = …`) |
| `inspect` | Structural read for explicit `paths[]` (nodes / params / errors / warnings; empty include defaults to nodes+errors+warnings) |
| `capture` | Perception (`top` / `preview` / `auto` / `chop_data`; `chop_image`/`pop` = preview aliases) |
| `describe_tools` | Tool manifest |

Process-scoped tools require `pid`. `inspect` takes a required non-empty
`paths` array (soft-capped at 32; no auto-recursion). Prefer
`detailLevel: summary` — each node’s direct-child roster is `name` + `opType`,
capped at 64 (`node.truncation` when truncated). Use `capture` when look is
the claim (`preview` rasterizes any family via the bridge’s shared OP Viewer
TOP); builders never self-grade perception.

## Docs

| Doc | Role |
| --- | --- |
| [`docs/CONTRACT.md`](docs/CONTRACT.md) | v1 contract, tools, OpPath, diagnostics, phases |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | Crate boundaries and topology |
| [`AGENTS.md`](AGENTS.md) | Agent route-first entry |
| [`CONSTITUTION.md`](CONSTITUTION.md) | Rust engineering law |
| [`docs/TESTING.md`](docs/TESTING.md) | Test strategy |
| [`docs/CONFIG.md`](docs/CONFIG.md) | TOML config file, Settings GUI, keep_alive / always_on |
| [`docs/DELIVERY.md`](docs/DELIVERY.md) | Packaging / install |
| [`docs/E2E_CHECKLIST.md`](docs/E2E_CHECKLIST.md) | Live TD verification |
| [`TODO_ENFORCE_TYPE.md`](TODO_ENFORCE_TYPE.md) | Typing / schema policy |

Local quality gate: `scripts/check.ps1` (Windows) or `scripts/check.sh` (Unix).
