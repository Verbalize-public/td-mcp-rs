# td-mcp-rs

A curated MCP control plane for TouchDesigner — not another thin wrapper. One
long-lived local daemon fronts any number of TouchDesigner instances and any
number of MCP clients (Cursor, Claude, …) over a resilient IPC link.

## What it does

**Shipped (v1)**

- **Multi-instance first** — every call addresses a TouchDesigner instance by
  OS `pid`. No sticky "current target", no generated peer ids.
- **Multiple MCP consumers** — Cursor and Claude (and other Streamable HTTP
  callers) share one daemon; the HTTP control plane stays up across MCP client
  restarts.
- **Resilient IPC** — local named-pipe / UDS link between daemon and TD, with
  heartbeat, per-`pid` task queues, and resurrection of cancelled tasks on
  reconnect.
- **Agent-shaped surface** — a small tool set built around a
  `fleet` → `inspect` → `capture` read model, plus `execute_python`,
  `mutate_nodes`, `api_help`, and `editor_context`; uniform rustc-style
  diagnostics; summary-by-default.
- **Context-aware** — `editor_context` reports which panes / owner COMPs /
  selection the user is looking at.
- **Self-contained delivery** — one binary (bridge, catalog, and bootstrap
  `.tox` embedded) + one drop-in `.tox`.
- **Cross-platform** — Windows, macOS, Linux. System-tray dashboard by
  default, or headless.

**Roadmap (not shipped)**

- `dialogs` — list / dismiss OS dialogs (planned, P1).
- Lifecycle create / start / stop of TD projects (planned, P2).
- Remote / multi-machine control (master–slave) — reserved, not v1.
- Offline `.toe` / `.tox` editing — out of scope for v1; the adopt path is the
  drop-in tox.

## Install

### 1. Build

Build the daemon from source:

```text
cargo build --release -p tdmcp-daemon
```

Binary: `target/release/tdmcp-daemon` (`.exe` on Windows), with the system-tray
dashboard included by default via the `gui` Cargo feature. Headless build:

```text
cargo build --release -p tdmcp-daemon --no-default-features
```

To build **and** assemble a release tree in one step (kills leftover daemon
processes, rebuilds with `gui`, copies into `target/dist/`):

```text
cargo run -p xtask -- dist
```

See [`docs/DELIVERY.md`](docs/DELIVERY.md) for packaging details.

### 2. Install assets + config

Run the install step once:

```text
# Windows
target\release\tdmcp-daemon.exe install

# macOS / Linux
target/release/tdmcp-daemon install
```

`install` is idempotent and does two things:

1. **Extracts the embedded assets** — `bridge/` (Python package),
   `diagnostics/catalog.yaml`, `bootstrap.tox`, and `skills/` (operate pack) —
   into the data dir (`%LOCALAPPDATA%\tdmcp-rs\` on Windows; Application Support /
   XDG data elsewhere). Add `--force` to re-extract even when the assets are
   already current.
2. **Resets `config.toml`** to the shipped defaults
   (`%APPDATA%\tdmcp-rs\config.toml` on Windows; Application Support / XDG
   config elsewhere).

Strictly, `install` is optional — `start`, `ensure`, and `mcp` all extract the
assets on first use. Run `install` when you want an explicit, clean setup or
to reset the config to defaults. Every setting lives in `config.toml`; see
[`docs/CONFIG.md`](docs/CONFIG.md).

## Quickstart

1. **Add to Cursor** — see [`mcp.tdmcp.example.json`](mcp.tdmcp.example.json);
   use an absolute path (on Unix omit `.exe`):

   ```json
   {
     "mcpServers": {
       "tdmcp-rs": {
         "command": "C:/absolute/path/to/tdmcp-daemon.exe",
         "args": ["mcp"]
       }
     }
   }
   ```

   Keep `mcp.json` minimal — settings live in `config.toml`, not in MCP
   `args` / `env`. See [`docs/CONFIG.md`](docs/CONFIG.md).

   Cursor spawns `tdmcp-daemon mcp`, which runs `ensure` (health → lock →
   detached spawn → poll), then serves a stdio MCP proxy to
   `http://127.0.0.1:9860/mcp/rpc` (port from config). The HTTP daemon stays
   up across MCP client restarts. By default it exits after **~30s** with no
   MCP sessions and no TD bridges; set `keep_alive = true` (or tray Settings)
   to disable that. The detached daemon shows a system-tray icon and a startup
   toast (dashboard hidden until you left-click the tray). **Stop** ends the
   process. Use `--no-gui` / `TDMCP_NO_GUI=1` / `show_tray = false` for
   headless.

2. **Drop the bootstrap into TouchDesigner** — load
   `%LOCALAPPDATA%\tdmcp-rs\bootstrap.tox` into a project (a thin dialer COMP
   `tdmcp_rs`). TD dials the local IPC endpoint, handshakes, and loads the
   bridge package from the path the daemon returns. The COMP Operator Viewer
   shows a color-banded status face + live task list; Bridge page pars are
   `Connect`, `Autoconnect`, `Status`, and `Cancelqueued`.

3. **Verify** — `GET http://127.0.0.1:9860/mcp/health` → `{"ok":true}`, then
   `fleet` should list the TD process with `bridge: "connected"`.

### Power users

Manual daemon + direct Streamable HTTP (no stdio shim):

```text
tdmcp-daemon start --port 9860
# Headless: tdmcp-daemon start --port 9860 --no-gui
# MCP client URL: http://127.0.0.1:9860/mcp/rpc
# Health: GET http://127.0.0.1:9860/mcp/health → {"ok":true}
```

Tray (default): icon + toast on start; left-click toggles the compact
dashboard (Docker-style; auto-hides on focus loss). Header: gear (Settings) ·
`.tox` · Restart · Stop. Right-click: Restart / Stop. Closing the window only
hides it — use **Stop** / `tdmcp-daemon stop` to end the process. Settings
edits write `config.toml` and apply after the next restart.

Other CLI helpers:

| Command | Job |
| --- | --- |
| `install [--force]` | Extract embedded assets **and** reset config to defaults |
| `ensure [--force]` | Spawn the daemon if down; `--force` re-extracts assets |
| `start [--port N]` | Start the daemon in the foreground |
| `status` | Print daemon health (`GET /mcp/health`) |
| `stop` | Ask a running daemon to shut down (`/admin/shutdown`) |
| `mcp` | Cursor/IDE entrypoint: ensure daemon, then speak MCP over stdio |

## Tools

| Tool | Job |
| --- | --- |
| `fleet` | Fleet view — processes by `pid`, bridge status, tasks, cancelled traces |
| `execute_python` | Run Python in TD (`result = …`); optional `logs`; structured `exception` on failure |
| `inspect` | Structural read for explicit `paths[]` (nodes / params / errors / warnings; no auto-recursion) |
| `mutate_nodes` | Ordered create / set / delete / connect / disconnect steps; sequential apply, stop on first hard error |
| `capture` | Perception — `top` / `preview` / `auto` / `chop_data` (`chop_image` / `pop` = `preview` aliases) |
| `api_help` | Live TD Python API cards (class / classes index / thin module) — not wiki/help dumps |
| `editor_context` | Live editor panes + per-pane selection (`ownerPath`, `focused`, `selection`) |
| `describe_tools` | Manifest of available tools |

MCP **resources** (`resources/list` / `resources/read`): operate docs under
`tdmcp://docs/*` (OpSketch, Python cheatsheet, DoD, primers, …). Prefer
resources over inventing TD procedure from memory.

Process-scoped tools require `pid`. `inspect` takes a required non-empty
`paths` array (soft-capped at 96; no auto-recursion). Prefer
`detailLevel: summary` — each node's direct-child roster is `name` + `opType`,
capped at 96 (`node.truncation` when truncated). `editor_context` returns all
panes (cap 32) with optional per-pane selection (cap 96). Use `capture` when
look is the claim (`preview` rasterizes any family via the bridge's shared OP
Viewer TOP); grade look via `tdmcp://docs/look-grade`. Full contract:
[`docs/CONTRACT.md`](docs/CONTRACT.md).

## Docs

| Doc | Role |
| --- | --- |
| [`docs/CONTRACT.md`](docs/CONTRACT.md) | v1 contract, tools, OpPath, diagnostics, phases |
| MCP resources `tdmcp://docs/*` | Operate pack — OpSketch, Python, DoD, primers, … |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | Crate boundaries and topology |
| [`AGENTS.md`](AGENTS.md) | Agent route-first entry |
| [`CONSTITUTION.md`](CONSTITUTION.md) | Rust engineering law |
| [`docs/CONFIG.md`](docs/CONFIG.md) | TOML config file, Settings GUI, keep_alive / always_on |
| [`docs/DELIVERY.md`](docs/DELIVERY.md) | Packaging / install |
| [`docs/TESTING.md`](docs/TESTING.md) | Test strategy |
| [`docs/E2E_CHECKLIST.md`](docs/E2E_CHECKLIST.md) | Live TD verification |
| [`docs/DEV_ENV.md`](docs/DEV_ENV.md) | Interactive dual-MCP dev harness |
| [`TODO_ENFORCE_TYPE.md`](TODO_ENFORCE_TYPE.md) | Typing / schema policy |

Local quality gate: `scripts/check.ps1` (Windows) or `scripts/check.sh` (Unix).
