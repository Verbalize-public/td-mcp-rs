<div align="center">

<img src="logo.svg" alt="td-mcp-rs logo" width="88">

# td-mcp-rs

### Your AI editor, inside your TouchDesigner project

**Cursor, Claude Code, or any MCP assistant — now with eyes and hands in the
network you're building. Live, local, and completely on your machine.**

[![License](https://img.shields.io/badge/License-MIT-green?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.92-000000?style=flat-square&logo=rust&logoColor=white)](https://rustup.rs)
[![Version](https://img.shields.io/badge/version-0.1.4-blue?style=flat-square)](CHANGELOG.md)
[![Platform](https://img.shields.io/badge/Windows%20%7C%20macOS-9cf?style=flat-square)](#-quick-start)

<img src="docs/screens/overview-populated.png" alt="td-mcp-rs dashboard — Overview with a live TouchDesigner fleet" width="820">

[What is it?](#-what-is-td-mcp-rs) · [Quick start](#-quick-start) · [The toolbox](#-the-toolbox) · [Screens](#-screens) · [Roadmap](#-roadmap)

</div>

---

## 👀 What is td-mcp-rs?

AI assistants write brilliant Rust and flawless SQL — but inside TouchDesigner
they've been flying blind. They can't see your network, can't tell a Noise TOP
from a Null, and every "fix my graph" request turns into you describing the
screen.

**td-mcp-rs closes that gap.** It's a small tray app that connects your editor
to a *running* TouchDesigner over a local MCP server. The assistant gets a
focused toolbox: look at any operator, create and wire nodes, run Python in
your project, take screenshots, and read built-in TouchDesigner primers — so
it stops guessing and starts operating.

```text
 ┌──────────────────┐    MCP over localhost     ┌──────────────────┐
 │  Your editor     │ ────────────────────────► │   td-mcp-rs      │
 │  Cursor / Claude │                           │   tray + daemon  │
 └──────────────────┘                           └────────┬─────────┘
                                                         │  local bridge
                                                         ▼
                                                ┌──────────────────┐
                                                │  TouchDesigner   │
                                                │  your project    │
                                                └──────────────────┘
```

Nothing leaves your machine. No cloud relay, no account, no telemetry — one
binary, one drop-in `.tox`, one local connection.

---

## 🚀 Quick start

> **Easiest path:** grab an installer from the
> [Releases page](https://github.com/Verbalize-public/td-mcp-rs/releases) —
> `tdmcp-rs-*-x64-setup.exe` for Windows or the `.dmg` for macOS. No Rust
> needed; skip to step 3. *Unsigned builds:* Windows may show SmartScreen
> (*More info → Run anyway*); on macOS right-click → **Open** the first time.

**Build from source instead:** you need [Rust](https://rustup.rs) and a
minute of patience —

```bash
cargo build --release -p tdmcp-daemon
target/release/tdmcp-daemon install        # Windows: target\release\tdmcp-daemon.exe install
```

`install` copies the app to a permanent folder, prepares the TouchDesigner
bridge file, and sets up autostart. Run it once and forget it.

### 3. Hook up your editor

**Claude Code** — two commands:

```text
/plugin marketplace add Verbalize-public/td-mcp-rs
/plugin install td-mcp-rs@td-mcp-rs
```

You'll be prompted once for the daemon path (the default is fine if it's on
`PATH`). This registers the MCP server *and* installs the TouchDesigner
operate skill — details in [`docs/CLAUDE_CODE_PLUGIN.md`](docs/CLAUDE_CODE_PLUGIN.md).

**Cursor** (or any MCP-compatible editor) — add to your MCP config, replacing
`<you>`:

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

macOS path: `~/Library/Application Support/tdmcp-rs/bin/tdmcp-daemon`
(see [`mcp.tdmcp.example.json`](mcp.tdmcp.example.json)).

### 4. Drop the bridge into TouchDesigner

Drag `bootstrap.tox` from the install folder into any project:

| OS | Path |
| --- | --- |
| Windows | `%LOCALAPPDATA%\tdmcp-rs\bootstrap.tox` |
| macOS | `~/Library/Application Support/tdmcp-rs/bootstrap.tox` |

A small `tdmcp_rs` component appears and connects on its own — its
color-coded status face tells you when the link is live.

### 5. Say hello

> *Connect to my TouchDesigner and show me what's in my project.*

Your project appears in the dashboard's fleet with a connected status. From
here, just ask: *"build a particle system with the Palette's particlesGpu"*
or *"why is this TOP black?"*.

<details>
<summary><b>Under the hood</b> — what's running, headless mode, CLI commands</summary>

- When your editor spawns `tdmcp-daemon mcp`, the app makes sure the
  background service is up, then connects on port `9860`. The service keeps
  running in the tray even if the editor closes.
- By default it stays resident (`keep_alive = true`); set `keep_alive = false`
  in `config.toml` (or Settings) to auto-exit when idle.
- **Tray:** left-click toggles a compact glance card, double-click opens the
  dashboard, right-click opens a menu (Dashboard · Stop · Restart). Closing
  windows only hides them — **Stop** (two-step confirm) is the real exit.
- **Headless:** `tdmcp-daemon start --port 9860 --no-gui` or `TDMCP_NO_GUI=1`.

| Command | What it does |
| --- | --- |
| `install [--force]` | Set up assets + reset config to defaults |
| `ensure [--force]` | Start the background service if it's not running |
| `start [--port N]` | Run the service in the foreground |
| `status` | Check if the service is healthy |
| `stop` | Shut the service down |
| `mcp` | Editor entrypoint (what your editor spawns) |
| `skills path` / `skills render --dest <dir>` | Where skill files live / export them |

</details>

---

## 🧰 The toolbox

Small, sharp, and agent-shaped — every tool returns structured, catalog-backed
diagnostics instead of strings to parse.

**See it**

| Tool | What the AI can do |
| --- | --- |
| `fleet` | Every running TouchDesigner, its project, and bridge health |
| `inspect` | Look inside any operator — params, errors, wiring, children |
| `capture` | Screenshot a TOP, preview *any* family, or read CHOP data |
| `editor_context` | See which network you have open and what's selected |

**Edit it**

| Tool | What the AI can do |
| --- | --- |
| `mutate_nodes` | Create, set, wire, delete — and drop Palette components — in one ordered batch, with rollback on failure |

**Run it**

| Tool | What the AI can do |
| --- | --- |
| `execute_python` | Run Python in your project and get results + captured prints |

**Run the whole studio**

| Tool | What the AI can do |
| --- | --- |
| `spawn_td` / `kill_td` | Start TouchDesigner on a project (deterministic pid) and close it |
| `dialogs` | See and dismiss popups that would block automation |
| `project_unpack` / `project_pack` | Expand a `.toe`/`.tox` to editable files and pack it back |
| `project_lint` / `project_install_bridge` | Sanity-check a project; inject or refresh the bridge inside it |
| `td_installs` | Find every TouchDesigner install and whether it's usable |

**Learn it**

| Tool | What the AI can do |
| --- | --- |
| `api_help` | Look up the TouchDesigner Python API while working |
| `palette_index` / `palette_probe` | Browse the Palette and learn what each component is for |
| skill pack | Built-in primers + references (network design, GLSL, OpSketch, …) served over MCP |

**Palette awareness.** TouchDesigner ships hundreds of ready-made components,
and you probably have your own. The AI can index them, probe their real
interfaces, and `place` one into your network in the same batch that wires it
up — so it reaches for `particlesGpu` instead of rebuilding a particle system
badly. Nothing about your palette ships with the tool: it builds that
knowledge on your machine, a slice at a time, and you can blacklist anything
you'd rather it never load. Say *"learn my palette"* to start.

<details>
<summary><b>Fine print</b> — caps and conventions for power users</summary>

- `inspect` takes a required `paths` array (soft-cap 256; no auto-recursion).
  `detailLevel: summary` returns child rosters as `{name, opType}`, capped at
  256 (`node.truncation` when truncated).
- Use `capture` for visual look claims; `preview` rasterizes any family via
  the bridge's shared OP Viewer TOP.
- Bridged tools are exclusive-enqueued per pid; a second overlapping call
  fails fast with `queue_busy` instead of interleaving.
- Full contract: [`docs/CONTRACT.md`](docs/CONTRACT.md).

</details>

---

## 🖼️ Screens

**Overview** — one page for everything: daemon health, your TouchDesigner
fleet grouped by machine, connected MCP clients, and recent activity.

<details>
<summary><b>More screens</b></summary>

| | |
| --- | --- |
| ![Overview empty state](docs/screens/overview-empty.png) | ![Daemon unreachable](docs/screens/overview-offline.png) |
| *Empty state — with one-click Reveal .tox* | *Offline — clear error, safe actions* |
| ![Add-slave modal](docs/screens/modal-add-slave.png) | ![Stop confirmation](docs/screens/stop-confirm.png) |
| *Federation: add-slave with built-in network scan* | *Destructive actions get a two-step confirm* |
| ![Logs](docs/screens/logs-filtered.png) | ![Settings](docs/screens/settings-dirty.png) |
| *Logs — filters, search, follow/pause* | *Settings — honest Save gating + restart hints* |

</details>

---

## 🔒 Private by design

- **Local only.** Editor ↔ daemon ↔ TouchDesigner talk over loopback. No
  telemetry, no accounts, no cloud.
- **pid-addressed.** Every call names an OS pid — no sticky sessions, no
  hidden global state.
- **You hold the blacklist.** Palette probing never touches your real
  project; components you blacklist are never loaded.
- **Structured failures.** Every error carries a stable `tdmcp.*` code with a
  mitigation hint — the AI fixes itself instead of asking you to paste logs.

---

## 🗺️ Roadmap

**Shipped (Windows + macOS)**

- [x] Multiple TouchDesigner instances, addressed by OS `pid`
- [x] One shared service for multiple editors (survives editor restarts)
- [x] Reliable local connection — heartbeat, resurrection of cancelled tasks, exclusive queue
- [x] TCP bridge transport — one `.tox` fits every OS
- [x] Agent-shaped tool set with catalog-backed diagnostics
- [x] Self-contained delivery — one binary + one drop-in `.tox`
- [x] Built-in operate skills (served over MCP, or rendered to files)
- [x] Tray dashboard with Logs / Settings, headless mode
- [x] `dialogs` — list / dismiss OS popups that block automation
- [x] Lifecycle — `spawn_td` / `kill_td` with deterministic pid ownership
- [x] Offline `.toe` / `.tox` editing via official toeexpand/toecollapse
- [x] Palette awareness — index, probe, place, blacklist

**In progress**

- [ ] Linux / Wine support — TCP transport done; lifecycle + packaging next
      ([`docs/OPEN_WORK.md`](docs/OPEN_WORK.md))

**Planned**

- [ ] Bounded payloads + artifact spool (large captures travel as files, not base64)
- [ ] Bridge token auth (today: loopback-only trust model)

---

<details>
<summary><b>🧑‍💻 For developers</b> — tech stack, repo layout, docs, contributing</summary>

**Tech stack**

| Layer | Technology |
| --- | --- |
| Core | Rust (edition 2021, MSRV 1.92) — tokio, axum, rmcp |
| MCP | Streamable HTTP server + stdio proxy (rmcp) |
| IPC | TCP loopback (`127.0.0.1:9861`), framing + handshake, heartbeat + task queues |
| GUI | eframe / egui + tray-icon / ksni (`gui` feature) |
| Config | TOML via `toml_edit` (`config.toml`) |
| Skills | Jinja templates (minijinja), embedded via `include_dir` |
| TD side | Embedded Python bridge package + drop-in `bootstrap.tox` |

**Repository layout**

```text
td-mcp-rs/
├── crates/
│   ├── tdmcp-core          # PidRegistry, shared types
│   ├── tdmcp-config        # config.toml load/save + Settings schema
│   ├── tdmcp-diagnostics   # rustc-style diagnostics + catalog.yaml
│   ├── tdmcp-ipc           # TCP loopback + framing + handshake
│   ├── tdmcp-mcp           # MCP server: tools, resources, stdio proxy
│   ├── tdmcp-projectio     # toeexpand/toecollapse, toc/sidecar, palette store
│   ├── tdmcp-dialogs       # OS dialogs (Win32 + UIA, macOS)
│   ├── tdmcp-daemon        # composition root: CLI, HTTP, tray
│   ├── tdmcp-gui           # egui dashboard (consumed via `gui` feature)
│   └── tdmcp-test-support  # shared test helpers
├── bridge/                 # Python package embedded into the daemon
├── skills/                 # Jinja skill templates + MANIFEST.yaml
├── claude-skills/          # Rendered skill pack for the Claude Code plugin (generated, checked in)
├── .claude-plugin/         # Claude Code plugin manifest
├── diagnostics/            # catalog.yaml
├── scripts/                # check.ps1 / check.sh quality gate
├── docs/                   # contract, config, delivery, testing, …
└── xtask/                  # packaging (cargo run -p xtask -- dist)
```

**Docs**

| Doc | Role |
| --- | --- |
| [`docs/CONTRACT.md`](docs/CONTRACT.md) | Contract of record — tools, shapes, diagnostics |
| [`docs/CONFIG.md`](docs/CONFIG.md) | `config.toml` reference, Settings GUI |
| [`docs/DELIVERY.md`](docs/DELIVERY.md) | Packaging / install / release tree |
| [`docs/TESTING.md`](docs/TESTING.md) | Test strategy |
| [`docs/E2E_CHECKLIST.md`](docs/E2E_CHECKLIST.md) | Live TouchDesigner acceptance rows |
| [`docs/DEV_ENV.md`](docs/DEV_ENV.md) | Day-to-day live-TD dev harness |
| [`docs/OPEN_WORK.md`](docs/OPEN_WORK.md) | What is not done yet (plans + deferred items) |
| [`docs/CLAUDE_CODE_PLUGIN.md`](docs/CLAUDE_CODE_PLUGIN.md) | Claude Code plugin — layout, skills render |
| [`ARCHITECTURE.md`](ARCHITECTURE.md) | Crate boundaries and topology |
| [`CONSTITUTION.md`](CONSTITUTION.md) | Rust engineering law (never-panic, lints) |
| [`RISKS.md`](RISKS.md) | Accepted panic/unsafe exceptions |
| [`AGENTS.md`](AGENTS.md) | Agent entry point |

**Contributing**

1. Fork the repo and create a feature branch.
2. Keep the quality gate green: `scripts/check.ps1` (Windows) or
   `scripts/check.sh` (Unix).
3. Read [`CONSTITUTION.md`](CONSTITUTION.md) first — never-panic is enforced:
   `unwrap_used`, `expect_used`, and `panic` are deny-by-default lints.
4. Open a PR against `main` and say what you verified (tests + live TD where
   it applies).

</details>

---

## 📄 License

MIT — declared in [`Cargo.toml`](Cargo.toml).
