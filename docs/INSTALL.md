# Install guide

Full setup for any MCP assistant. The short version is the
[README quick start](../README.md#quick-start).

There are three pieces:

```text
  1. the daemon          2. your assistant           3. the bridge
  (a tray app on         (told to run                (a .tox dropped into
   your machine)          `tdmcp-daemon mcp`)         your TD project)
```

**Contents**

- [Before you start](#before-you-start)
- [Step 1 — install the daemon](#step-1--install-the-daemon)
- [Where the binary lands](#where-the-binary-lands)
- [Step 2 — connect your assistant](#step-2--connect-your-assistant)
  · [Claude Code](#claude-code) · [Cursor](#cursor)
  · [VS Code + Copilot](#vs-code--github-copilot) · [Codex CLI](#codex-cli)
  · [Windsurf](#windsurf) · [Zed](#zed)
  · [Claude Desktop](#claude-desktop) · [Cline / Roo Code](#cline--roo-code)
  · [Continue](#continue) · [Any other MCP client](#any-other-mcp-client)
- [Step 3 — install the bridge in TouchDesigner](#step-3--install-the-bridge-in-touchdesigner)
- [Step 4 — verify it works](#step-4--verify-it-works)
- [Updating](#updating)
- [Uninstalling](#uninstalling)
- [Troubleshooting](#troubleshooting)

---

## Before you start

| You need | Notes |
| --- | --- |
| **TouchDesigner** | Any recent build, Windows or macOS. The free non-commercial licence is fine. |
| **An AI assistant that speaks MCP** | Claude Code, Cursor, Codex CLI, VS Code + Copilot, Windsurf, Zed, Cline, Continue… anything on the list below, and plenty that aren't. |
| **Windows 10/11 or macOS 12+** | Linux is in progress — see [`LINUX_SUPPORT.md`](LINUX_SUPPORT.md). |
| **[Rust](https://rustup.rs)** | Only if you build from source. Not needed for the installers. |

You do **not** need: an account, an API key from us, a subscription, an
internet connection at runtime, or admin rights.

---

## Step 1 — install the daemon

Pick one path.

### Option A — the installers (easiest)

Download the newest build from the
[**Releases page**](https://github.com/Verbalize-public/td-mcp-rs/releases).

**Windows**

1. Download `tdmcp-rs-<version>-x64-setup.exe`.
2. Run it. It installs per-user, so no admin prompt.
3. If Windows SmartScreen appears, click **More info → Run anyway**.

**macOS**

1. Download `tdmcp-rs-<version>-<arch>.dmg` — `aarch64` for Apple Silicon,
   `x86_64` for Intel.
2. Open it and drag **tdmcp** to Applications.
3. The first launch: **right-click the app → Open → Open**. (Double-clicking
   will be blocked.) If macOS still refuses, run once in Terminal:
   ```bash
   xattr -cr /Applications/tdmcp.app
   ```

> The builds aren't code-signed or notarized yet, which is what triggers
> those warnings. Signing is on the [roadmap](../README.md#roadmap).

### Option B — from source with Cargo

Works on every platform, and puts the binary on your `PATH`, so every editor
config below is a single word instead of a long path.

1. Install Rust from [rustup.rs](https://rustup.rs), then **close and reopen
   your terminal** so `cargo` is on your `PATH`.

2. Build and install:

   ```bash
   git clone https://github.com/Verbalize-public/td-mcp-rs
   cd td-mcp-rs
   cargo install --path crates/tdmcp-daemon
   ```

   This takes a few minutes the first time. It puts `tdmcp-daemon` in
   `~/.cargo/bin` (Windows: `%USERPROFILE%\.cargo\bin`), which rustup already
   added to your `PATH`.

3. Set up the assets and the TouchDesigner bridge:

   ```bash
   tdmcp-daemon install
   ```

   This unpacks the Python bridge, the diagnostics catalog, the operate manual
   and `bootstrap.tox` into your data directory, resets `config.toml` to
   defaults, and records the binary path for restarts and autostart.

4. Check it:

   ```bash
   tdmcp-daemon --version
   ```

> **Headless machines** (render nodes with no desktop session) can skip the
> tray entirely: `cargo install --path crates/tdmcp-daemon --no-default-features`
> builds a binary with no GUI linked in at all.

> Package-manager installs are planned; `cargo` is the source path for now.

### Option C — a plain build, no install

For development, or if you don't want anything copied anywhere:

```bash
cargo build --release -p tdmcp-daemon
target/release/tdmcp-daemon install     # Windows: target\release\tdmcp-daemon.exe install
```

`install` copies the binary to `{data_dir}/bin/` so your editor points at a
stable location instead of a build artifact you might delete.

### Starting it

After any of the above, start the daemon once:

```bash
tdmcp-daemon start
```

A tray icon appears. **Left-click** for a glance card, **double-click** for the
dashboard, **right-click** for the menu.

You rarely need to do this by hand: your editor starts the daemon on its first
tool call. To start it with your computer, open **Settings → Always on** in the
dashboard and restart the daemon.

---

## Where the binary lands

You'll need this path for editors that can't find `tdmcp-daemon` on your
`PATH`.

| How you installed | Windows | macOS |
| --- | --- | --- |
| **Installer** | `%LOCALAPPDATA%\Programs\tdmcp-rs\tdmcp-daemon.exe` | `/Applications/tdmcp.app/Contents/MacOS/tdmcp-daemon` |
| **`cargo install`** | `%USERPROFILE%\.cargo\bin\tdmcp-daemon.exe` | `~/.cargo/bin/tdmcp-daemon` |
| **`tdmcp-daemon install`** | `%LOCALAPPDATA%\tdmcp-rs\bin\tdmcp-daemon.exe` | `~/Library/Application Support/tdmcp-rs/bin/tdmcp-daemon` |

Not sure? Ask your shell:

```bash
which tdmcp-daemon        # macOS / Linux
where tdmcp-daemon        # Windows
```

Most editors don't expand `~` or `%LOCALAPPDATA%` inside config files —
**write the full absolute path**, e.g.
`/Users/you/.cargo/bin/tdmcp-daemon` or
`C:/Users/you/.cargo/bin/tdmcp-daemon.exe`. Forward slashes work on Windows
too, and save you escaping backslashes.

---

## Step 2 — connect your assistant

Each of these does the same thing: run `tdmcp-daemon` with the single argument
`mcp`. No port, no token, no project path. Wherever you see `tdmcp-daemon`
below, substitute the full path from the table above if it isn't on your
`PATH`.

> Editors move their config files between versions. If a path here doesn't
> match what you see, use the editor's own **Add MCP server** UI — the setting
> is always "command + arguments".

### Claude Code

**The plugin (recommended).** It registers the MCP server and installs the
TouchDesigner skill pack, so Claude reaches for the right tool without being
told:

```text
/plugin marketplace add Verbalize-public/td-mcp-rs
/plugin install td-mcp-rs@td-mcp-rs
```

Claude Code prompts once for the `tdmcp-daemon` binary. If you used
`cargo install`, the default `tdmcp-daemon` is already right — press enter.
Otherwise paste the path from [the table above](#where-the-binary-lands).

Verify with `/mcp` — you should see `tdmcp-rs` connected — and `/plugin` to
confirm the skill is active.

**Without the plugin**, from your shell:

```bash
claude mcp add tdmcp-rs -- tdmcp-daemon mcp
```

This gives you the tools but not the skill pack. Prefer the plugin.

Details: [`CLAUDE_CODE_PLUGIN.md`](CLAUDE_CODE_PLUGIN.md).

### Cursor

**Settings → Cursor Settings → MCP → Add new global MCP server.** That opens
`~/.cursor/mcp.json` (Windows: `%USERPROFILE%\.cursor\mcp.json`). Add:

```json
{
  "mcpServers": {
    "tdmcp-rs": {
      "command": "tdmcp-daemon",
      "args": ["mcp"]
    }
  }
}
```

For one project only, use `.cursor/mcp.json` in the project folder instead.

Reopen the MCP settings page — `tdmcp-rs` should show a green dot and a tool
count. Then chat in **Agent** mode; Ask mode can't call tools.

### VS Code + GitHub Copilot

Command Palette (<kbd>Ctrl/Cmd</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd>) →
**MCP: Add Server…** → **Command (stdio)** → command `tdmcp-daemon`, argument
`mcp`, name `tdmcp-rs`. Choose **Global** to make it available everywhere.

Or write it yourself in `.vscode/mcp.json` (note: the key is `servers`, not
`mcpServers`):

```json
{
  "servers": {
    "tdmcp-rs": {
      "type": "stdio",
      "command": "tdmcp-daemon",
      "args": ["mcp"]
    }
  }
}
```

Open **Copilot Chat** and switch the mode dropdown to **Agent**. The tools
icon should list the td-mcp-rs tools.

### Codex CLI

Register the local stdio server with Codex:

```bash
codex mcp add tdmcp-rs -- tdmcp-daemon mcp
```

If Codex cannot find `tdmcp-daemon` on its `PATH`, use the full path from
[Where the binary lands](#where-the-binary-lands), for example:

```bash
codex mcp add tdmcp-rs -- \
  /home/you/.local/share/tdmcp-rs/bin/tdmcp-daemon mcp
```

The command writes the global entry to `~/.codex/config.toml`. To configure it
manually instead, add:

```toml
[mcp_servers.tdmcp-rs]
command = "tdmcp-daemon"
args = ["mcp"]
```

Note the underscore in `mcp_servers` — it's TOML, not JSON.

Verify the registration with:

```bash
codex mcp list
codex mcp get tdmcp-rs
```

In the Codex TUI, `/mcp` shows the active server after restarting the session.
The server is local and does not require OAuth.

### Windsurf

**Settings → Cascade → MCP Servers → Manage / View raw config**, which opens
`~/.codeium/windsurf/mcp_config.json`:

```json
{
  "mcpServers": {
    "tdmcp-rs": {
      "command": "tdmcp-daemon",
      "args": ["mcp"]
    }
  }
}
```

Click **Refresh** in the MCP panel afterwards.

### Zed

Open `settings.json` (Command Palette → **zed: open settings**) and add a
custom context server:

```json
{
  "context_servers": {
    "tdmcp-rs": {
      "source": "custom",
      "command": "tdmcp-daemon",
      "args": ["mcp"],
      "env": {}
    }
  }
}
```

Zed's agent settings panel has an **Add Custom Server** button that writes
this for you. Prefer it — Zed has changed this schema before.

### Claude Desktop

Edit the config file:

| OS | Path |
| --- | --- |
| macOS | `~/Library/Application Support/Claude/claude_desktop_config.json` |
| Windows | `%APPDATA%\Claude\claude_desktop_config.json` |

(Or **Settings → Developer → Edit Config**.)

```json
{
  "mcpServers": {
    "tdmcp-rs": {
      "command": "tdmcp-daemon",
      "args": ["mcp"]
    }
  }
}
```

Fully quit and reopen Claude Desktop — a hammer/tools icon appears in the
chat box when it has connected.

> Claude Desktop launches without your shell's environment, so `PATH` lookups
> often fail here. Use the **full absolute path** to the binary.

### Cline / Roo Code

In the VS Code sidebar, open Cline (or Roo) → the **MCP Servers** icon →
**Configure MCP Servers**. That opens `cline_mcp_settings.json`:

```json
{
  "mcpServers": {
    "tdmcp-rs": {
      "command": "tdmcp-daemon",
      "args": ["mcp"],
      "disabled": false
    }
  }
}
```

### Continue

Add to `~/.continue/config.yaml`:

```yaml
mcpServers:
  - name: tdmcp-rs
    command: tdmcp-daemon
    args: ["mcp"]
```

MCP tools are only used in Continue's **Agent** mode.

### Any other MCP client

If it speaks MCP over stdio, it works. You're describing the same thing in
that client's own dialect:

| Field | Value |
| --- | --- |
| Name | `tdmcp-rs` |
| Type / transport | `stdio` (command, not URL/SSE) |
| Command | `tdmcp-daemon` (or its absolute path) |
| Arguments | `mcp` |
| Environment | *(none needed)* |

There is also a **Streamable HTTP** endpoint at `http://127.0.0.1:9860/mcp`
if your client prefers URLs. With it you must start the daemon yourself
(`tdmcp-daemon start`), since nothing launches it on demand. The stdio command
handles that for you.

A copy-paste starting point lives at
[`mcp.tdmcp.example.json`](../mcp.tdmcp.example.json).

---

## Step 3 — install the bridge in TouchDesigner

The daemon reaches TouchDesigner through a small component inside the project.
One drag per project.

### The manual way

1. Open the dashboard (double-click the tray icon) and click **Reveal .tox** —
   your file browser opens on the file. Or go there yourself:

   | OS | Path |
   | --- | --- |
   | Windows | `%LOCALAPPDATA%\tdmcp-rs\bootstrap.tox` |
   | macOS | `~/Library/Application Support/tdmcp-rs/bootstrap.tox` |

2. **Drag `bootstrap.tox` into a TouchDesigner network.** A component named
   `tdmcp_rs` appears.

3. Its face turns green within a second or two. Red or amber means it can't
   reach the daemon — see [Troubleshooting](#troubleshooting).

4. **Save the project.** The bridge is part of it from now on.

With the daemon stopped the component just retries quietly and costs nothing,
so it is safe to leave in a project you ship.

### The automatic way

Once any assistant is connected, ask it:

> *"Install the tdmcp bridge into `~/projects/show.toe`."*
>
> *"Install the bridge into every .toe in this folder."*

That's the `project_install_bridge` tool. It works on closed project files,
backs up the original first, and verifies the result. No TouchDesigner window
needed.

You can also have the assistant create projects that already contain it, by
pointing `[project] template_path` in `config.toml` at a template `.toe` with
the bridge inside. Then *"make me a new project"* just works. See
[`CONFIG.md`](CONFIG.md#project-template-create-new).

---

## Step 4 — verify it works

1. Start TouchDesigner with a project containing the bridge.
2. Open the td-mcp-rs dashboard. Under **TOUCHDESIGNER**, your project should
   be listed with its process id and **connected** in green.
3. In your assistant, say:

   > *"Use td-mcp-rs: list the TouchDesigner instances, then show me what's
   > inside my project."*

You should get back a fleet listing with a real process id, then a description
of your network using operator names you recognise.

Then the fuller test:

> *"Add a Noise TOP and a Level TOP inside a new COMP called `test_tdmcp`,
> wire them together, then screenshot the result and tell me what you see."*

If it builds the network, looks at it, and describes the image, everything is
working.

---

## Updating

**Installers:** download the new one and run it — it replaces in place.

**Cargo:**

```bash
cd td-mcp-rs
git pull
cargo install --path crates/tdmcp-daemon --force
tdmcp-daemon install --force
```

`install --force` re-extracts the bridge and the operate manual, and **resets
`config.toml` to defaults** — note down any settings you've changed first.

The bridge inside your saved projects updates itself: the `.tox` is only a
dialer and reloads the Python package from disk on every connection. You don't
need to re-drag it after an update.

If a daemon is already running, `install` restarts it onto the new binary. If
the file is locked (Windows), stop the daemon from the tray (**Stop**) and
re-run.

---

## Uninstalling

1. **Stop** from the tray menu (two-step confirm).
2. Remove the binary: Windows *Add or remove programs* → **tdmcp-rs**; macOS
   drag `/Applications/tdmcp.app` to the Trash; Cargo users
   `cargo uninstall tdmcp-daemon`.
3. Remove the MCP server entry from your editor's config.
4. Optional — delete the data and config directories:

   | | Windows | macOS |
   | --- | --- | --- |
   | Config | `%APPDATA%\tdmcp-rs\` | `~/Library/Application Support/tdmcp-rs/config.toml` |
   | Data / logs | `%LOCALAPPDATA%\tdmcp-rs\` | `~/Library/Application Support/tdmcp-rs/` |

5. The `tdmcp_rs` component inside your projects is harmless if left, but you
   can delete it like any other operator.

---

## Troubleshooting

### My editor doesn't list any td-mcp-rs tools

- **Restart the editor** after editing its config. Most only read MCP config
  at startup.
- Check you're in the mode that can call tools — Cursor **Agent**, Copilot
  **Agent**, Continue **Agent**. Ask/Chat modes can't.
- Check the JSON is valid — a trailing comma silently disables the whole file
  in most editors.
- Run the command by hand: `tdmcp-daemon mcp`. It should sit waiting for
  input — that's correct, press <kbd>Ctrl</kbd>+<kbd>C</kbd>. If it says
  *command not found*, your editor can't find it either: use the absolute
  path.

### "command not found" / the editor can't launch the binary

GUI apps don't inherit your shell's `PATH`. Use the full absolute path from
[Where the binary lands](#where-the-binary-lands). On Windows, include the
`.exe` and use forward slashes.

### `tdmcp.daemon.unreachable`

The background service isn't running or is restarting. Check with:

```bash
tdmcp-daemon status
```

If it's down, run `tdmcp-daemon start`. The stdio proxy reconnects once the
daemon is healthy, so your next tool call should succeed. If the daemon keeps
dying, `tdmcp-daemon logs 100` says why.

### The daemon won't start — port already in use

Something else holds port `9860`. Stop it, or change the port in **Settings →
Port** (or `[server] port` in `config.toml`) and restart. Your editor config
doesn't change; it never mentions the port.

### The `tdmcp_rs` component in TouchDesigner never turns green

- Is the daemon actually running? Check the tray icon or `tdmcp-daemon status`.
- The bridge connects on `127.0.0.1:9861`. A local firewall or endpoint
  security tool blocking loopback will break it — allow `tdmcp-daemon`.
- Open TouchDesigner's **Textport** (Alt+T / ⌥T) and look for `tdmcp` lines —
  they name the failure.
- Check the dashboard's **Logs** page with filter `bridge`.

### My TouchDesigner doesn't appear in the fleet

- The bridge component must be **in the open project** — check you saved after
  dragging it in.
- Multiple TouchDesigner instances each need their own bridge; each one shows
  as its own process id.
- A project that was open *before* you installed the daemon needs the
  component dropped in and the project saved.

### Everything hangs, or I get `tdmcp.dialog.blocking`

TouchDesigner has a modal dialog open — a build-upgrade prompt, a missing-file
warning, a crash report. Nothing reaches it until that's dismissed. Ask your
assistant:

> *"What dialogs are open on that pid? Dismiss them."*

Startup dialogs are never auto-dismissed: "would you like to save?" is not the
tool's call to make.

**macOS:** describing and dismissing dialogs needs Accessibility permission.
**System Settings → Privacy & Security → Accessibility** → enable
`tdmcp-daemon`. Listing works without it.

### `queue_busy`

Two tool calls tried to use the same TouchDesigner at once. TouchDesigner has
one main thread, and interleaving would corrupt state. The assistant should
retry; if it's looping, tell it to make one call at a time.

### The assistant gives good TouchDesigner advice but never calls a tool

It doesn't know the tools apply here. Say so explicitly:

> *"Use the td-mcp-rs tools. Call `fleet` first, then `inspect`."*

On Claude Code, install the **plugin** rather than the bare MCP server; the
bundled skill pack is what makes tool use automatic.

### Something else

- `tdmcp-daemon logs 200` — the tail of the daemon log, human-readable.
- Dashboard → **Logs** — filter and search live, including bridge and
  assistant traffic.
- Every error carries a stable `tdmcp.*` code. Search this repo for it to find
  the meaning in [`diagnostics/catalog.yaml`](../diagnostics/catalog.yaml).
- Still stuck? [Open an issue](https://github.com/Verbalize-public/td-mcp-rs/issues)
  with the code, the log tail, and your OS.

---

## Next

- **[Recipes](RECIPES.md)** — what to say, from one-liners to full builds.
- **[Federation](FEDERATION.md)** — controlling several machines from one seat.
- **[Config reference](CONFIG.md)** — every setting explained.
