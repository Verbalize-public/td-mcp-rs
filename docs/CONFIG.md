# Configuration

td-mcp-rs reads a curated TOML file as the source of truth for daemon settings.
Cursor / IDE `mcp.json` should stay minimal (`args: ["mcp"]`) — do not put port,
idle, or path overrides there.

## File location

| OS | Path |
| --- | --- |
| Windows | `%APPDATA%\tdmcp-rs\config.toml` |
| macOS | `~/Library/Application Support/tdmcp-rs/config.toml` |
| Linux | `$XDG_CONFIG_HOME/tdmcp-rs/config.toml` or `~/.config/tdmcp-rs/config.toml` |

The **data directory** (bridge, catalog, bootstrap `.tox`) stays separate under
the OS local data dir (`%LOCALAPPDATA%\tdmcp-rs\` on Windows, etc.). The config
path never depends on a value inside the config file.

Internal / test override: set `TDMCP_CONFIG_PATH` to an absolute file path so
tests never touch the user config.

## Creating and resetting

| Action | Config file behavior |
| --- | --- |
| First `start` / `ensure` / `mcp` | Create-if-missing from the embedded template |
| `tdmcp-daemon install` (any) | **Always** overwrite with the shipped defaults |
| `install --force` | Same config reset + re-extract embedded assets |
| Tray Settings → Reset | Force-write defaults (same template) |

The template is `crates/tdmcp-config/assets/default.toml`, embedded via
`include_str!`.

## Precedence

1. CLI flags / env (`--port`, `TDMCP_PORT`, `--data-dir`, `TDMCP_DATA_DIR`,
   `--bridge-dir`, `--catalog`, `--no-gui` / `TDMCP_NO_GUI`)
2. TOML config file
3. Built-in defaults

`keep_alive` and `always_on` are **config/GUI only** (no CLI flags).

`TDMCP_IDLE_EXIT_SECS` remains a test/escape hatch for the idle timeout length
(`0` disables idle exit even when `keep_alive = false`; irrelevant when
`keep_alive = true` since idle exit is already disabled).

### Stdio proxy per-call ceilings (env only)

The stdio proxy (`tdmcp-daemon mcp`) bounds every forwarded call with a
wall-clock ceiling so a wedged daemon session surfaces as an error instead of
hanging the MCP client forever. Defaults sit above the `[bridge]` budgets
(45s / 120s) so live calls are never cut early. On expiry the proxy heals the
link (fresh session) and returns `tdmcp.daemon.unreachable` with `budgetMs`.

| Env | Default | Meaning |
| --- | --- | --- |
| `TDMCP_PROXY_CALL_TIMEOUT_MS` | `105000` | Ceiling for short tools (`inspect` / `capture` / `fleet` / …) |
| `TDMCP_PROXY_SCRIPT_TIMEOUT_MS` | `180000` | Ceiling for `execute_python` / `mutate_nodes` |
| `TDMCP_PROXY_LIST_TIMEOUT_MS` | `30000` | Ceiling for `tools/list` |

## Fields

### `[server]`

| Key | Default | Meaning |
| --- | --- | --- |
| `port` | `9860` | HTTP listen port (MCP + admin) |
| `bind_address` | `127.0.0.1` | Listen address. Use `0.0.0.0` for LAN remote access. Non-loopback requires `[auth] mode = "psk"` with a non-empty `psk`. |

### `[auth]`

| Key | Default | Meaning |
| --- | --- | --- |
| `mode` | `none` | `none` (no Bearer) or `psk` (`Authorization: Bearer`). |
| `psk` | `""` | Shared secret for incoming MCP + federation + remote `/admin/config`. |

### `[federation]`

| Key | Default | Meaning |
| --- | --- | --- |
| `role` | `standalone` | `standalone` \| `master` \| `slave`. |
| `daemon_id` | *(auto UUID)* | Stable daemon identity; generated on first start; do not copy across machines. |
| `master_url` | `""` | Slave only: master base URL (e.g. `http://192.168.1.100:9860`). |
| `master_psk` | `""` | Slave only: PSK to present to the master (master’s `auth.psk`). |

`role = "slave"` disables idle auto-exit (same effect as `keep_alive`).

### Federation auth & admin surface

**Auth matrix**

| Hop | Bearer |
| --- | --- |
| Slave → master register / fleet-push | Master's `auth.psk` (slave config field `master_psk`) |
| Client → any daemon `/mcp/rpc` | That daemon's `auth.psk` (skip if `mode=none`) |
| Master → slave tool proxy | Slave's `auth.psk` as register body `authToken` (empty if slave `mode=none`); stored in `SlaveRegistry` |
| Master → slave `/admin/config` | Same stored `authToken` |

`master_psk` meaning: PSK to present to the master (the master's `auth.psk`).

**Middleware allowlist**

| Path | Remote (`0.0.0.0`) | Auth |
| --- | --- | --- |
| `/mcp/rpc`, `/mcp/health`, `/mcp/tools/*` | Allowed | Bearer if `auth.mode=psk` |
| `/admin/federation/*` (except status probe) | Allowed | Bearer if psk |
| `/admin/config` GET/POST | Allowed | Bearer if psk |
| `/admin/federation/status` | Allowed | **Unauth minimal probe** `{ok,version,role,hostname,daemonId,port}` |
| `/admin/status` | Loopback only (or auth) | Not the LAN scan oracle |
| `/admin/shutdown`, `/admin/restart`, `/admin/mcp-sessions*` | **Loopback only** | N/A remote |

`daemon_id` conflict: re-registering the same `daemonId` from the same advertised
base URL overwrites; from a different host/port it is rejected with a diagnostic.

### `[daemon]`

| Key | Default | Meaning |
| --- | --- | --- |
| `keep_alive` | `true` | When `true`, never auto-exit after idle (no MCP sessions and no TD bridges). When `false`, idle exit uses ~30s (or `TDMCP_IDLE_EXIT_SECS`). |
| `always_on` | `false` | When `true`, register OS login autostart for `tdmcp-daemon start`. Reconciled once at daemon start. |
| `show_tray` | `true` | When `false`, run headless (gui builds). CLI `--no-gui` still forces headless. |

### `[bridge]`

IPC call budgets and idle heartbeat. Changes apply after the next daemon restart.
`idle_dead_secs` is also forwarded to connecting Python bridges via handshake
`idleDeadSecs` so both sides share the same silence budget.

| Key | Default | Meaning |
| --- | --- | --- |
| `call_timeout_secs` | `45` | Wait for `ping` / `inspect` / `capture` responses |
| `script_timeout_secs` | `120` | Wait for `execute_python` / `mutate_nodes` |
| `heartbeat_interval_secs` | `5` | Idle bridge ping cadence |
| `pong_timeout_secs` | `8` | Max wait for a heartbeat pong |
| `idle_dead_secs` | `20` | Tear down after this much inbound silence |

A call timeout fails the **wait** (`tdmcp.bridge.timeout`); it does not tear down
the bridge. Stale late responses are discarded on the next call so they cannot
masquerade as `tdmcp.bridge.lost`.

### `[logging]`

Central JSONL sink (`docs/OBSERVABILITY_PLAN.md` M1). `dir`/`filter` are
optional overrides — omit to use the defaults below. `filter` precedence for
the file layer is `[logging].filter` > `RUST_LOG` > built-in default
(`info,tdmcp_daemon=debug`); `console_level` follows the same precedence for
the stderr layer, falling back to the historical per-target defaults when
unset. An invalid explicit filter falls through to the next source rather
than failing startup.

| Key | Default | Meaning |
| --- | --- | --- |
| `dir` | *(unset)* | Log directory override; unset = `{data_dir}/logs` |
| `filter` | *(unset)* | `EnvFilter` string for the file layer |
| `max_files` | `14` | Daily rotated files kept on disk |
| `retention_days` | `30` | Sweep threshold (startup + every 24h) |
| `console_level` | *(unset)* | Separate `EnvFilter` for the stderr layer |

`tdmcp-daemon logs [N]` prints the tail of the newest `daemon.*.log` in the
resolved directory, human-formatted (`HH:MM:SS.SSS LEVEL SRC TARGET msg
{kvs}`) — the JSONL files themselves are the machine-readable format.
There is deliberately no `TDMCP_LOG` env var — `RUST_LOG` plus `[logging]`
cover the need.

### `[advanced]`

Optional path overrides. Omit (or leave blank in the GUI) to use defaults under
the data directory.

| Key | Default | Meaning |
| --- | --- | --- |
| `data_dir` | *(unset)* | Install / data root |
| `bridge_dir` | *(unset)* | Python bridge package directory |
| `catalog_path` | *(unset)* | `diagnostics/catalog.yaml` path |
| `daemon_bin` | *(unset)* | Path to the installed daemon binary (auto-set by `install`; used for spawn / restart / autostart instead of `current_exe()`) |

## Keep alive vs idle exit

Idle exit (when not keep-alive) cancels a shared shutdown token and sets the
process quit flag; the composition root drains axum and ends on the main thread.
Background / tokio paths do **not** call `process::exit`.

## Always on (autostart)

When `always_on` is true at daemon start, the process registers itself with the
OS login mechanism via the `auto-launch` crate:

| OS | Mechanism |
| --- | --- |
| Windows | Current-user Run registry key |
| macOS | Launch Agent |
| Linux | XDG autostart (`.desktop`) |

Turning `always_on` off and restarting removes the registration. Changes are not
applied until the next start.

## Settings GUI

1. Left-click the tray icon to open the dashboard.
2. Click **⚙** (gear) in the header.
3. Edit fields → **Save** or **Discard** (both return to the fleet view).
4. **Reset** rewrites the file from the shipped template.
5. Restart the daemon for changes to take effect.

## Example

```toml
[server]
port = 9860

[daemon]
keep_alive = true
always_on = false
show_tray = true

[bridge]
call_timeout_secs = 45
script_timeout_secs = 120
heartbeat_interval_secs = 5
pong_timeout_secs = 8
idle_dead_secs = 20

[advanced]
# data_dir = "C:/path/to/tdmcp-rs-data"
# daemon_bin = "C:/path/to/tdmcp-daemon.exe"
```

## Project I/O & dialogs (v2)

```toml
[official_tools]
# Pin Derivative's official tools for offline project I/O.
# td_exe / expand_path / collapse_path (expand+collapse must be set together)

[dialogs]
enabled   = true   # popup watcher + `dialogs` tool (Windows)
intercept = true   # fail bridged calls fast while a modal blocks TD
poll_ms   = 1000
```

Absence of `[official_tools]` triggers env (`TDMCP_TOEEXPAND`,
`TDMCP_TOECOLLAPSE`, `TDMCP_TOUCHDESIGNER_EXE`) then a Program Files scan that
validates actual tool files (stub installs are skipped).
