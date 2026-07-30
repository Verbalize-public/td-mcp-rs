# td-mcp-rs

Rust control plane for live TouchDesigner MCP. Multi-instance first: every call
addresses a TD process by OS `pid`. No sticky target, no offline `.toe` editing.

## Install

Build from source (release packaging lands later):

```text
cargo build --release -p tdmcp-daemon -p tdmcp-gui
```

Binaries: `target/release/tdmcp-daemon`, `target/release/tdmcp-gui`.
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
   restarts.

   First run extracts embedded bridge, catalog, and bootstrap `.tox` into the
   data dir (`install` / `ensure` / `start` / `mcp` all do this). Default:
   `%LOCALAPPDATA%/tdmcp-rs/` (Windows), Application Support (macOS), or XDG
   (Linux).

3. Drop the bootstrap tox into a TouchDesigner project (or load
   `bridge/bootstrap.py`). TD dials the local IPC endpoint, handshakes, and
   loads the bridge package from the path the daemon returns.

4. Optional operator UI:

   ```text
   tdmcp-gui
   ```

   Tray menu: show/hide dashboard, restart/stop daemon, quit.

### Power users

Manual daemon + direct Streamable HTTP (no stdio shim):

```text
tdmcp-daemon start --port 9860
# MCP client URL: http://127.0.0.1:9860/mcp/rpc
# Health: GET http://127.0.0.1:9860/mcp/health → {"ok":true}
```

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
