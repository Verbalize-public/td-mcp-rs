# Linux and Wine

The daemon runs natively on Linux; TouchDesigner runs through Wine.
A working TD installation is a prerequisite, not something the daemon installs.

## Supported behavior

- TCP bridge on loopback, MCP/admin server, federation, and dashboard.
- TD discovery in common Wine prefixes and an explicitly configured location.
- Spawn and PID-based stop; official project tools invoked through Wine.
- Capture, inspect, mutate, and Python once the TD bridge connects.

Native TD dialog enumeration/dismissal is unavailable on Linux and returns
an unsupported-platform diagnostic. Inspect TD's visible windows when a modal
blocks a call. Graceful stop requests process termination; forced stop may
discard unsaved changes.

## Configure the runner

The daemon scans `WINEPREFIX`, the default prefix, common wineprefix directories,
and the touchdesigner-linux prefix. Override discovery when needed:

```toml
[official_tools]
wine_exe = "/absolute/path/to/wine-or-wrapper"
wine_prefix = "/absolute/path/to/prefix"
td_exe = "/absolute/path/to/TouchDesigner.exe"
```

The equivalent runner/prefix environment variables are `TDMCP_WINE_EXE`
and `TDMCP_WINE_PREFIX`. Restart after changing runner configuration.

A wrapper receives the Windows executable as its first argument, followed by
the executable's arguments. Use `exec` to preserve the launch PID. The runner
must work for both TD and Derivative's command-line expand/collapse tools.
Do not assume system Wine is interchangeable with a bundled runner:
CodeMeter, graphics settings, and prefix setup may differ.

For touchdesigner-linux, a wrapper can delegate to its installed launcher:

```sh
#!/usr/bin/env bash
exec /absolute/path/to/touchdesigner --no-patch --exe "$@"
```

Use the actual launcher path and make the wrapper executable. Confirm those
options against the launcher installed on your machine.

## Verify and troubleshoot

1. Launch TD normally through the intended runner and confirm it opens.
2. Start the daemon and install the bootstrap in a scratch project.
3. Read MCP `fleet`; inspect and capture using its Linux PID.
4. Test daemon-driven `spawn_td` on a separate scratch file.

The bridge exports the Linux launch PID because Wine's Windows PID is not a
valid native process target. Avoid launchers that fork away and abandon that PID.

If `spawn_td` times out, check whether the original process is alive before
trying again. Inspect its window, launcher output, and daemon Logs. A stuck
splash screen can be a runner or licensing issue, not an MCP failure.

The GUI uses a DBus StatusNotifierItem tray without GTK/libappindicator.
On desktops without tray support, use the dashboard launch command or run
headless. Linux DBus consumers use a consistent async-io backend to avoid
accessibility-thread runtime panics.
