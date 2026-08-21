<div align="center">

# td-mcp-rs 🎛️

**A curated MCP control plane for TouchDesigner — not another thin wrapper.**

![License](https://img.shields.io/badge/License-MIT-green?style=for-the-badge)
![Rust](https://img.shields.io/badge/Rust-1.88-000000?style=for-the-badge&logo=rust&logoColor=white)
![Version](https://img.shields.io/badge/version-0.1.1-blue?style=for-the-badge)
![MCP](https://img.shields.io/badge/MCP-Streamable_HTTP-7C3AED?style=for-the-badge)
![Platform](https://img.shields.io/badge/Windows%20%7C%20macOS%20%7C%20Linux-9cf?style=for-the-badge)

**One long-lived local daemon fronts any number of TouchDesigner instances for
any number of MCP clients — Cursor, Claude, or anything speaking Streamable
HTTP — over a resilient named-pipe / UDS link.**

<!-- TODO: add a screenshot or GIF — tray dashboard + a TD session driven through MCP -->

[About](#-about) · [Features](#-features) · [Quick Start](#-quick-start) · [Tools](#-tools) · [Tech Stack](#-tech-stack) · [Docs](#-docs) · [Roadmap](#-roadmap) · [Contributing](#-contributing) · [License](#-license)

</div>

---

## 💡 About

> A curated control plane, not a thin wrapper around the TouchDesigner Python API.

td-mcp-rs gives AI coding agents a safe, agent-shaped way to work *inside*
TouchDesigner. Every call addresses a specific TD instance by OS `pid`, runs
through one long-lived local daemon that survives MCP client restarts, and
lands on a small tool set built around the `fleet → inspect → capture` read
model agents actually need. A drop-in `.tox` dialer plus an embedded Python
bridge do the heavy lifting inside TouchDesigner itself.

---

## ✨ Features

| Feature | What it gives you |
| --- | --- |
| **Multi-instance first** | Address any TouchDesigner instance by OS `pid` — no sticky "current target", no generated peer ids |
| **Any MCP client** | Cursor, Claude, and other Streamable HTTP callers share one daemon; it stays up across client restarts |
| **Resilient IPC** | Named-pipe / UDS link between daemon and TD, with heartbeat, per-`pid` task queues, and resurrection of cancelled tasks on reconnect |
| **Agent-shaped tools** | `fleet` → `inspect` → `capture` read model plus `execute_python`, `mutate_nodes`, `api_help`, `editor_context` |
| **Context-aware** | `editor_context` reports which panes / owner COMPs / selection the user is looking at |
| **Operate skills** | Jinja-templated TouchDesigner skill pack (primers + references) served as MCP resources or rendered to disk |
| **Self-contained** | One binary with the bridge, diagnostics catalog, bootstrap `.tox`, and skill templates embedded |
| **Cross-platform** | Windows, macOS, Linux — system-tray dashboard by default, headless on request |

---

## 🚀 Quick Start

**You need:** Rust 1.88+ ([`rustup`](https://rustup.rs)) and TouchDesigner
installed. Everything else is pulled in and embedded at build time.

### 1. Build the daemon

```text
cargo build --release -p tdmcp-daemon
```

The binary lands at `target/release/tdmcp-daemon` (`.exe` on Windows). The
system-tray dashboard is compiled in by default via the `gui` feature.

### 2. Install assets + register the binary

```text
# Windows
target\release\tdmcp-daemon.exe install

# macOS / Linux
target/release/tdmcp-daemon install
```

`install` is idempotent and does three things:

1. **Extracts the embedded assets** — `bridge/` (Python package),
   `diagnostics/catalog.yaml`, `bootstrap.tox`, and the skill templates — into
   the data dir (`%LOCALAPPDATA%\tdmcp-rs\` on Windows; Application Support /
   XDG data elsewhere). Add `--force` to re-extract.
2. **Resets `config.toml`** to the shipped defaults (`%APPDATA%\tdmcp-rs\`
   on Windows; Application Support / XDG config elsewhere).
3. **Copies the daemon binary** to `{data_dir}/bin/tdmcp-daemon[.exe]` and
   records it in `config.toml`, so IDE-spawned processes never lock the build
   artifact in `target/release/`.

Strictly, `install` is optional — `start`, `ensure`, and `mcp` all extract
assets on first use. Run it when you want a clean, explicit setup. Every
setting lives in `config.toml`; see [`docs/CONFIG.md`](docs/CONFIG.md).

### 3. Point an MCP client at it (Cursor)

Add a server entry to your MCP client config — copy
[`mcp.tdmcp.example.json`](mcp.tdmcp.example.json) and use the **absolute
path** to the installed binary (drop `.exe` on Unix):

```json
{
  "mcpServers": {
    "tdmcp-rs": {
      "command": "C:/Users/<you>/AppData/Local/tdmcp-rs/bin/tdmcp-daemon.exe",
      "args": ["mcp"]
    }
  }
}
```

Cursor spawns `tdmcp-daemon mcp`, which makes sure the long-lived daemon is up
(`ensure`: health → lock → detached spawn → poll), then serves a stdio MCP
proxy to `http://127.0.0.1:9860/mcp/rpc`. Keep the MCP config minimal — all
settings live in `config.toml`, not in `args` / `env`.

### 4. Drop the bootstrap into TouchDesigner

Load `%LOCALAPPDATA%\tdmcp-rs\bootstrap.tox` into a project — it's a thin
dialer COMP (`tdmcp_rs`) that dials the daemon's IPC endpoint, handshakes, and
loads the bridge package from the path the daemon returns. The COMP Operator
Viewer shows a color-banded status face and the live task list; bridge page
pars: `Connect`, `Autoconnect`, `Status`, `Cancelqueued`.

### 5. Verify

```text
tdmcp-daemon status                  # → {"ok":true}
```

Then call `fleet` from the MCP client — your TouchDesigner process should list
with `bridge: "connected"`.

<details>
<summary><b>Power users</b> — manual daemon, headless mode, tray dashboard</summary>

Run the daemon directly and speak Streamable HTTP without the stdio shim:

```text
tdmcp-daemon start --port 9860
# Headless:   tdmcp-daemon start --port 9860 --no-gui
# MCP client: http://127.0.0.1:9860/mcp/rpc
# Health:     GET http://127.0.0.1:9860/mcp/health → {"ok":true}
```

By default the daemon exits after ~30s with no MCP sessions and no TD bridges;
set `keep_alive = true` in `config.toml` (or tray Settings) to disable that.
The tray dashboard (default) shows an icon + startup toast on start; left-click
toggles the compact dashboard (Docker-style, auto-hides on focus loss),
right-click offers Restart / Stop. Closing the window only hides it — use
**Stop** or `tdmcp-daemon stop` to end the process. Headless: `--no-gui` /
`TDMCP_NO_GUI=1` / `show_tray = false`.

Other CLI commands:

| Command | Job |
| --- | --- |
| `install [--force]` | Extract embedded assets **and** reset config to defaults |
| `ensure [--force]` | Spawn the daemon if down; `--force` re-extracts assets |
| `start [--port N]` | Run the daemon in the foreground |
| `status` | Print daemon health (`GET /mcp/health`) |
| `stop` | Ask a running daemon to shut down (`/admin/shutdown`) |
| `mcp` | Cursor/IDE entrypoint: ensure daemon, then speak MCP over stdio |
| `skills path` | Print the extracted skills dir (`{dataDir}/skills`) |
| `skills render --dest <dir>` | Render the Jinja skill templates to files |

Build **and** assemble a release tree in one step (kills leftover daemons,
rebuilds with `gui`, copies into `target/dist/`):

```text
cargo run -p xtask -- dist
```

Packaging details: [`docs/DELIVERY.md`](docs/DELIVERY.md).

</details>

---

## 🛠️ Tools

The MCP surface is small on purpose — every tool is process-scoped by `pid`:

| Tool | Job |
| --- | --- |
| `fleet` | Fleet view — processes by `pid`, bridge status, tasks, cancelled traces |
| `inspect` | Structural read for explicit `paths[]` (nodes / params / errors / warnings; no auto-recursion) |
| `execute_python` | Run Python in TD (`result = …`); optional `logs`; structured `exception` on failure |
| `mutate_nodes` | Ordered create / set / delete / connect / disconnect steps; sequential apply, stop on first hard error |
| `capture` | Perception — `top` / `preview` / `auto` / `chop_data` (`chop_image` / `pop` = `preview` aliases) |
| `api_help` | Live TD Python API cards (class / classes index / thin module) — not wiki/help dumps |
| `editor_context` | Live editor panes + per-pane selection (`ownerPath`, `focused`, `selection`) |
| `describe_tools` | Manifest of available tools |

MCP **resources** (`resources/list` / `resources/read`) serve the operate
skill pack under `tdmcp://docs/*` — OpSketch, Python cheatsheet, primers,
Definition of Done, look-grade, tooling-concurrency, and more. Prefer
resources over inventing TD procedure from memory. The same Jinja templates can
be rendered to plain files with `tdmcp-daemon skills render --dest <dir>`.

> [!NOTE]
> `inspect` takes a required non-empty `paths` array (soft-capped at 96; no
> auto-recursion). Prefer `detailLevel: summary` — each node's direct-child
> roster is `name` + `opType`, capped at 96 (`node.truncation` when
> truncated). Use `capture` when look is the claim (`preview` rasterizes any
> family via the bridge's shared OP Viewer TOP). Full contract:
> [`docs/CONTRACT.md`](docs/CONTRACT.md).

---

## 🏗️ Tech Stack

| Layer | Technology |
| --- | --- |
| Core | Rust (edition 2021, MSRV 1.88) — tokio, axum, rmcp |
| MCP | Streamable HTTP server + stdio proxy (rmcp) |
| IPC | Local named pipes / Unix domain sockets, heartbeat + task queues |
| GUI | eframe / egui + tray-icon (system-tray dashboard, `gui` feature) |
| Config | TOML via `toml_edit` (`config.toml`) |
| Skills | Jinja templates (minijinja), embedded via `include_dir` |
| TD side | Embedded Python bridge package + drop-in `bootstrap.tox` |

<details>
<summary><b>Repository layout</b></summary>

```text
td-mcp-rs/
├── crates/
│   ├── tdmcp-core          # PidRegistry, shared types
│   ├── tdmcp-config        # config.toml load/save + Settings schema
│   ├── tdmcp-diagnostics   # rustc-style diagnostics + catalog.yaml
│   ├── tdmcp-ipc           # named-pipe/UDS protocol, heartbeat, queues
│   ├── tdmcp-mcp           # MCP server: tools, resources, stdio proxy
│   ├── tdmcp-daemon        # composition root: CLI, HTTP, tray
│   ├── tdmcp-gui           # egui dashboard (consumed via `gui` feature)
│   └── tdmcp-test-support  # shared test helpers
├── bridge/                 # Python package embedded into the daemon
├── skills/                 # Jinja skill templates + MANIFEST.yaml
├── diagnostics/            # catalog.yaml
├── scripts/                # check.ps1 / check.sh quality gate
├── docs/                   # contract, config, delivery, testing, …
└── xtask/                  # packaging (cargo run -p xtask -- dist)
```

</details>

---

## 📚 Docs

| Doc | Role |
| --- | --- |
| [`docs/CONTRACT.md`](docs/CONTRACT.md) | v1 contract — tools, OpPath, diagnostics, phases |
| [`docs/CONFIG.md`](docs/CONFIG.md) | `config.toml`, Settings GUI, `keep_alive` / `always_on` |
| [`docs/DELIVERY.md`](docs/DELIVERY.md) | Packaging / install / release tree |
| [`docs/TESTING.md`](docs/TESTING.md) | Test strategy |
| [`docs/E2E_CHECKLIST.md`](docs/E2E_CHECKLIST.md) | Live TouchDesigner verification |
| [`docs/DEV_ENV.md`](docs/DEV_ENV.md) | Interactive dual-MCP dev harness |
| [`docs/CURATED_REVIEW.md`](docs/CURATED_REVIEW.md) | Architecture / stability review |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | Crate boundaries and topology |
| [`CONSTITUTION.md`](CONSTITUTION.md) | Rust engineering law (never-panic, lints) |
| [`RISKS.md`](RISKS.md) | Accepted panic/unsafe exceptions |
| [`AGENTS.md`](AGENTS.md) | Agent route-first entry |

MCP resources `tdmcp://docs/*` — operate pack (OpSketch, Python, DoD, primers,
…). Local quality gate: `scripts/check.ps1` (Windows) or `scripts/check.sh`
(Unix).

---

## 🗺️ Roadmap

**Shipped (v1)**

- [x] Multi-instance addressing by OS `pid`
- [x] One shared daemon for multiple MCP clients (survives client restarts)
- [x] Resilient named-pipe / UDS IPC — heartbeat + task resurrection
- [x] Agent-shaped tool set (`fleet` / `inspect` / `capture` + friends)
- [x] Self-contained delivery — one binary + one drop-in `.tox`
- [x] Jinja-templated operate skills (MCP resources + filesystem render)
- [x] Cross-platform daemon — tray dashboard / headless modes

**Planned**

- [ ] `dialogs` — list / dismiss OS dialogs (P1)
- [ ] Lifecycle — create / start / stop TD projects (P2)
- [ ] Remote / multi-machine master–slave control (reserved, not v1)
- [ ] Offline `.toe` / `.tox` editing (out of scope for v1; the adopt path is
      the drop-in tox)

---

## 🤝 Contributing

1. Fork the repo and create a feature branch (`feature/your-thing`).
2. Keep the quality gate green: `scripts/check.ps1` (Windows) or
   `scripts/check.sh` (Unix).
3. Read [`CONSTITUTION.md`](CONSTITUTION.md) before touching Rust code —
   never-panic is enforced: `unwrap_used`, `expect_used`, and `panic` are
   deny-by-default lints.
4. Open a PR against `main` and say what you verified (tests + live TD where
   it applies).

---

## 📄 License

MIT — declared in [`Cargo.toml`](Cargo.toml).
