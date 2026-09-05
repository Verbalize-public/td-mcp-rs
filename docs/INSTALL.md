# Install

You need TouchDesigner and an assistant that supports MCP.
Linux additionally needs a working Wine/TD installation; see
[Linux/Wine](LINUX_SUPPORT.md).

## Install the daemon

Download from [Releases](https://github.com/Verbalize-public/td-mcp-rs/releases):

- **Windows:** run the x64 setup executable. Installation is per-user.
- **macOS:** use the DMG matching Apple Silicon or Intel and move the app to
  Applications. Unsigned/ad-hoc-signed downloads may require approval in
  System Settings → Privacy & Security. Verify the download's source first.
- **Linux or portable use:** extract the matching archive, then run
  `./tdmcp-daemon install` and `./tdmcp-daemon ensure`.

For source builds (Rust 1.92+):

```sh
cargo build --locked --release --workspace
./target/release/tdmcp-daemon install
./target/release/tdmcp-daemon ensure
```

On Windows, use the corresponding `.exe` paths. `install` prints the stable
installed binary path; use it in your assistant. Normal startup is
`tdmcp-daemon ensure`; `start --no-gui` runs headless.

## Connect your assistant

Register a **stdio** MCP server with:

```json
{
  "mcpServers": {
    "touchdesigner": {
      "command": "/absolute/path/to/tdmcp-daemon",
      "args": ["mcp"]
    }
  }
}
```

The outer configuration format depends on your assistant. The command and
arguments are the same; use an absolute path and the Windows `.exe` suffix
where appropriate. The `mcp` command starts the HTTP daemon if necessary and
proxies the assistant's connection. Do not substitute `start` for `mcp`.

The [Claude Code plugin](CLAUDE_CODE_PLUGIN.md) provides registration and
operating skills together. Other clients receive the skill cards through MCP
resources; filesystem-only clients can use:

```sh
tdmcp-daemon skills render --dest ./td-skills
```

Restart the assistant's MCP connection after changing its server configuration.

## Add the bridge to a project

1. Open the dashboard and choose **Reveal .tox**.
2. Drag `bootstrap.tox` into the TD project. The component is named `tdmcp_rs`.
3. Wait for its connected indication and save the project.

The asset is in the runtime data directory:

| OS | Default data directory |
| --- | --- |
| Windows | `%LOCALAPPDATA%/tdmcp-rs/` |
| macOS | `~/Library/Application Support/tdmcp-rs/` |
| Linux | `~/.local/share/tdmcp-rs/` (or `$XDG_DATA_HOME/tdmcp-rs/`) |

Alternatively, ask the assistant to install the bridge into a **closed**
project using `project_install_bridge`. It uses Derivative's official tools
and backs up the original. Do not rewrite a project currently open in TD.

## Verify

Ask: “List TouchDesigner processes, inspect /project1 in the connected process,
and capture its output.” An empty fleet means no TD processes were discovered;
a disconnected row means TD was found but its bridge has not connected.
Tool calls use the returned PID, not a remembered active project.

## Updates

Run the newer installer, or run the newer binary's `install` command.
Configuration, identity, and customized starter projects are preserved.
`install --force` refreshes managed assets for a same-version development build;
it is not a settings reset. Restart/reconnect TD when changing bridge Python so
an existing session loads the new code.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| No tools listed | Absolute executable path, `mcp` argument, assistant MCP logs |
| Daemon unreachable | `tdmcp-daemon status`; Logs; port 9860 conflicts |
| TD absent | Installation/process discovery; on Linux use the intended Wine runner |
| TD disconnected | Bridge component present; daemon/bridge port both 9861; no modal blocking TD |
| Calls time out | TD dialogs, Logs, bridge budgets and independent proxy ceilings |
| Remote host absent | Both directions reachable; listener sharing; correct coordinator URL and keys |

After three identical failed probes, stop retrying and inspect new evidence.
Do not spawn duplicate TD processes to work around a handshake timeout:
the original process may still be starting.

See [Configuration](CONFIG.md), [Federation](FEDERATION.md), and
[Logs](OBSERVABILITY.md). For uninstalling, remove the OS application or
installed executable and assistant registration. Remove config/data separately
only if you intend to discard settings, logs, and customized assets.
