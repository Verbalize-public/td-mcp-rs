# td-mcp-rs

Rust control plane for live TouchDesigner MCP. Multi-instance first: every call
addresses a TD process by OS `pid`. No sticky target, no offline `.toe` editing.

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

   Cursor spawns `tdmcp-daemon mcp`, which runs `ensure` (health → lock →
   detached spawn → poll), then serves a stdio MCP proxy to
   `http://127.0.0.1:9860/mcp/rpc`. The HTTP daemon stays up across MCP client
   restarts. By default the detached daemon also shows a system-tray icon and
   a startup toast (dashboard window stays hidden until you open it from the
   tray; **Stop** ends the process). Use `--no-gui` or `TDMCP_NO_GUI=1` for
   headless.

   First run extracts embedded bridge, catalog, and bootstrap `.tox` into the
   data dir (`install` / `ensure` / `start` / `mcp` all do this). Default:
   `%LOCALAPPDATA%/tdmcp-rs/` (Windows), Application Support (macOS), or XDG
   (Linux).

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

Tray (default): icon + toast on start; open the dashboard yourself via Show /
tray click. Closing the window only hides it — use **Stop** /
`tdmcp-daemon stop` to end the process.

Other CLI helpers: `tdmcp-daemon ensure` (spawn if down), `install` (extract
assets only), `status`, `stop`.

## Tools (P0)

| Tool | Job |
| --- | --- |
| `fleet` | List TD processes by `pid`, bridge status, tasks, resurrection traces |
| `execute_python` | Run Python in TD (`result = …`) |
| `inspect` | Structural subtree read (nodes / params / errors) |
| `capture` | Perception (`top` / `preview` / `auto`) |
| `describe_tools` | Tool manifest |

Process-scoped tools require `pid`. Prefer `detailLevel: summary`. Use
`capture` when look is the claim; builders never self-grade perception.

## Docs

| Doc | Role |
| --- | --- |
| [`docs/CONTRACT.md`](docs/CONTRACT.md) | v1 contract, tools, OpPath, diagnostics, phases |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | Crate boundaries and topology |
| [`AGENTS.md`](AGENTS.md) | Agent route-first entry |
| [`CONSTITUTION.md`](CONSTITUTION.md) | Rust engineering law |
| [`docs/TESTING.md`](docs/TESTING.md) | Test strategy |
| [`docs/DELIVERY.md`](docs/DELIVERY.md) | Packaging / config |
| [`docs/E2E_CHECKLIST.md`](docs/E2E_CHECKLIST.md) | Live TD verification |
| [`TODO_ENFORCE_TYPE.md`](TODO_ENFORCE_TYPE.md) | Typing / schema policy |

Local quality gate: `scripts/check.ps1` (Windows) or `scripts/check.sh` (Unix).
