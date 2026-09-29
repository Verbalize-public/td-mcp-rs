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
- Retained evidence is written and readable by the client that asked for it, but
  only `inspect` captures, only in the daemon's own store, and only over the HTTP
  transport — there is no tool to page a retained observation, no cross-machine
  grant (`grant`/`revoke_grant` have no caller-facing route), and no export to a
  caller-chosen destination. Grant plumbing and the export path are tested at the
  store layer only. Retention is bounded by the store's quota and lease; there is
  no UI to inspect or pin what a session retained, so an observation is found
  again only by the id from the original reply.
- Third-party notices are generated only for the four supported release targets.
  Adding a target or dependency requires regenerating its locked graph inventory;
  the dependency gate rejects drift and any unreviewed license. Before publication,
  the publisher must confirm whether generated bindings from Plasma's LGPL-licensed
  protocol XML are treated as LGPL-covered output. The package conservatively
  preserves terms/source and gives rebuild/relink instructions, but the final
  tagged project-source archive still must be matched to the shipped binary and
  retained with the release. Native Windows/macOS installer acceptance remains a
  platform-specific release check.

Report a reproducible failure with daemon/TD versions, OS, affected PID,
the tool request, and relevant logs. Do not include keys or private projects.
