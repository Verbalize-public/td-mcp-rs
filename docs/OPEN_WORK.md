# Current limitations

These are known constraints, not a delivery schedule.

- Linux runs Windows TD through Wine. Runner/GPU/CodeMeter compatibility
  depends on the installation; native dialog inspection/dismissal is unavailable.
  See [Linux/Wine](LINUX_SUPPORT.md).
- Capture images travel inline through MCP. Individual DAT/shader text previews
  are capped at 64 KiB. Replies over the 32 MiB IPC budget fail without dropping
  the bridge. Recording has a bounded per-job artifact spool/read API, but
  there is no universal preview/pagination policy for arbitrary large results.
- Timed capture/record requires explicit TOP outputs; no audio or timed shared
  viewers. Initial recording is qtrle MOV (up to 4096×4096 / 512 MiB), tested on
  TD 2025.32460 under Wine/Linux. Native Windows/macOS and other codecs need
  separate acceptance. A recorder is a temporary sibling of the source, so its
  parent must permit operator creation. Container verification is not a decoder.
- Timing jobs reserve TD transport across calls but are not long-running rows
  in `fleet.tasks`; poll capture/record status. Retention is eight jobs per
  bridge generation, and reload invalidates jobs/private artifacts. The local
  Python deadline cannot preempt a natively blocked main thread.
- Bridge call budgets are live, but stdio proxy ceilings remain independent
  environment settings. See [Configuration](CONFIG.md#stdio-proxy-ceilings).
- Federation is one coordinator with directly joined computers, not a mesh.
  Scanning is local IPv4 /24 discovery, not routing or firewall configuration.
- Fleet connection status does not expose the TD main-thread pump's age.
- Some oversized admin/federation HTTP requests return plain HTTP 413 instead
  of the MCP diagnostic envelope.
- Windows signing and macOS Developer ID/notarization need publisher
  credentials. Native release installers must be verified on their own OS.
- Windows setup still stops processes by executable name. Path-scoped upgrade
  shutdown and cleanup of autostart registrations on uninstall need native
  verification before changing this behavior.
- The dependency gate currently rejects licenses already present in the GUI
  dependency tree (font licenses, BSL-1.0, CC0-1.0, Unlicense, and MPL-2.0).
  Distribution notices and explicit license-policy decisions are still needed.
  Linux's Wayland titlebar dependency also uses the unmaintained `ttf-parser`
  (`RUSTSEC-2026-0192`); it has not been suppressed in the gate.

Report a reproducible failure with daemon/TD versions, OS, affected PID,
the tool request, and relevant logs. Do not include keys or private projects.
