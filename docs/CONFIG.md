# Configuration

Use **Settings** for daemon options and **Federation** for connections.
Save validates changes before writing; Discard restores your last loaded
settings. Reset changes the draft only—Save is still required. Reset retains
the daemon identity and installed executable path.

## Settings you are most likely to need

| I want to… | Setting or page |
| --- | --- |
| Start new projects with my usual network | `project.template_path`: a `.toe` containing the bridge |
| Include my component collection | `palette.user_root` |
| Connect another computer | The **Federation** page; follow the [setup guide](FEDERATION.md) |
| Allow a slow script more time | `bridge.script_timeout_secs`; also check the proxy ceiling below |
| Start td-mcp-rs when I log in | `daemon.always_on` |
| Run without the tray/dashboard | `daemon.show_tray = false` |

You can leave defaults alone for a first local connection. The dashboard
reports which saved changes need a restart; save your TD work before arranging
one. Keep-alive controls idle exit, not automatic restart after every failure.

![Settings showing unsaved changes and a pending restart banner](screens/captures-2026-10-29/settings-dirty.png)

*Settings preview: saving a change and applying a restart are separate steps.
The port and configuration path shown are fixture values.*

## Files and precedence

| OS | Configuration | Runtime data |
| --- | --- | --- |
| Windows | `%APPDATA%/tdmcp-rs/config.toml` | `%LOCALAPPDATA%/tdmcp-rs/` |
| macOS | `~/Library/Application Support/tdmcp-rs/config.toml` | `~/Library/Application Support/tdmcp-rs/` |
| Linux | `$XDG_CONFIG_HOME/tdmcp-rs/config.toml` (default `~/.config/`) | `$XDG_DATA_HOME/tdmcp-rs/` (default `~/.local/share/`) |

`TDMCP_CONFIG_PATH` selects another file. Explicit CLI/environment overrides
take precedence over file settings, then built-in defaults.

Installation creates missing configuration and preserves existing values,
including with `install --force`. Saves use atomic replacement and retain
comments and unknown TOML keys. Malformed files are reported, not replaced.

The [commented default file](../crates/tdmcp-config/assets/default.toml)
is the full field reference. Do not edit `federation.daemon_id`: it identifies
this computer across restarts.

## When changes take effect

| Saved through the dashboard or admin API | Effect |
| --- | --- |
| Federation role, coordinator URL/key | Reconnect automatically |
| Bridge call and script timeout | New calls use the new budget, including connected TD processes |
| Keep alive | Next idle check |
| Project template and palette settings | Read by the relevant operation |
| HTTP/bridge listener, authentication | Restart required |
| Tray, autostart, paths, logging, dialogs, official tools | Restart required |
| Bridge heartbeat/pong/idle-dead settings | Restart required |
| `bridge.admission_max_image_bytes` | Restart required |

Manual edits to the file require a restart. Pending restart settings are
compared against the running daemon's startup settings; reverting them removes
the restart requirement. A restart disconnects MCP/bridge sessions temporarily.

## Common settings

- `server.port`: HTTP MCP/admin port, default **9860**.
- `server.bind_address`: default `127.0.0.1`; `0.0.0.0` permits LAN access.
- `auth.mode`: `none` or `psk`; `auth.psk` is the incoming Bearer key.
- `daemon.keep_alive`: default true. False allows idle exit when no clients,
  bridges, or federation role need the daemon.
- `daemon.always_on`: register startup at user login for the standard per-user
  configuration. Custom-config/test daemons do not modify that global entry;
  register their explicit launch command separately if needed.
- `daemon.show_tray`: default **true**, for every entry point including the
  `mcp` command your assistant spawns. Set false to run headless; the
  `--no-gui` flag and `TDMCP_NO_GUI=1` do the same per invocation. A host with
  no desktop session serves headless either way and reports `noGui: true`.
- `bridge.port`: default **9861**, loopback only; the bootstrap must dial it.
- `bridge.call_timeout_secs`: default **45**.
- `bridge.script_timeout_secs`: default **120**.
- `bridge.admission_max_image_bytes`: default **8388608** (8 MiB) of *encoded*
  image. A capture image above this is withheld from the MCP attachment list
  and reported in the result's `admission` block; its bytes stay in
  `structuredContent.imageBase64`. `0` is refused (it would withhold every
  image) and the default is kept. Derived from rmcp's 16 MiB client-side SSE
  event cap — a safe bound, not a tuned one. See
  [Contract](CONTRACT.md#mcp-tool-result-shapes-stdio--rmcp).
  The default admits all nine PNGs in the recorded sample, but that sample
  doesn't cover every image format or client. See [measurement scope](TESTING.md#a-measured-default-and-what-the-measurement-is-not)
  before tuning it; a larger server limit does not raise your client's limits.
- `project.template_path`: optional starter .toe; otherwise
  `{dataDir}/template.toe`. New-project creation never overwrites its target.
- `official_tools`: optional TD/expand/collapse paths and Linux Wine overrides.
  Set expand and collapse paths together.
- `palette`: optional user folder, store directory, and ignored component globs.

Keep-alive and autostart are different: one prevents idle exit; the other
starts the daemon when you log in. See [Linux/Wine](LINUX_SUPPORT.md) for
runner configuration and [Federation](FEDERATION.md) for networking.

## Stdio proxy ceilings

The stdio proxy also bounds calls to recover from wedged HTTP sessions.
Its environment-only ceilings are independent of bridge settings:
`TDMCP_PROXY_CALL_TIMEOUT_MS` and `TDMCP_PROXY_SCRIPT_TIMEOUT_MS`.
If you raise bridge budgets beyond those ceilings, raise the proxy ceilings
too and restart the assistant's MCP connection. Check
[daemon_link.rs](../crates/tdmcp-mcp/src/daemon_link.rs) for exact defaults.

## Admin API

`GET /admin/config` returns configuration. `POST /admin/config` accepts a
partial object using snake_case or camelCase field names. Unknown fields,
duplicate aliases, invalid types, and invalid values are rejected before
writing. Success includes `config` and a `restartRequired` list.

```json
{"bridge": {"scriptTimeoutSecs": 180}, "daemon": {"keepAlive": true}}
```

Treat returned configuration as sensitive: it contains access keys.
The endpoint is subject to the daemon's admin authentication policy.

## Diagnostic retention

`[logging]` defaults to `max_files = 180`, `retention_days = 180`,
`crash_max_reports = 500`, and `crash_tail_lines = 200`. Zero disables the
corresponding automatic cleanup limit (or omits the crash tail). Changes require
restart. Existing installations keep their configured limits during upgrades.
