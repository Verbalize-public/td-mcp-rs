# Changelog

## v0.2.0 — unreleased

112 commits against v0.1.4. It is **additive**: every tool,
argument and response field that existed in v0.1.4 still works, and a call
that omits `output` still gets the full legacy shape. Read
[docs/MIGRATION.md](docs/MIGRATION.md) before upgrading — it says what is new,
what is newly *labelled*, and what this release does not claim.

**The version is bumped in source but not yet published.** There is no `v0.2.0`
tag and nothing has been pushed; every artifact from this tree is UNSIGNED, and
native Windows/macOS installers have not been built or accepted. See
[docs/MIGRATION.md](docs/MIGRATION.md#what-this-release-does-not-claim) and
`docs/OPEN_WORK.md`.

### The GUI is the default, including from the IDE

- fix: `tdmcp-daemon mcp` — the entrypoint your assistant spawns — spawned the
  daemon headless by default and **ignored** `--no-gui`/`TDMCP_NO_GUI`/`show_tray`
  when deciding, so the tray was reachable only by launching the binary by hand.
  It now asks for the tray like `start` and `ensure` do, honouring
  `[daemon].show_tray`; opt back out with `--no-gui`, `TDMCP_NO_GUI=1`, or
  `show_tray = false`
- fix: a headless host is no longer a reason to fail or to restart-loop. A build
  without the `gui` feature, or a host with no reachable display session, serves
  headless deliberately and reports `noGui: true` — the truth it is serving in.
  `ensure_daemon` therefore settles instead of tearing down and respawning
  "with tray" a daemon that could never show one, on every IDE reconnect

### Measuring cost, honestly

- feat(mcp): count what was generated, emitted and admitted separately — a
  result carrying an image now reports four distinct numbers, and
  `includedInModelInput` is `unavailable` rather than assumed, because the
  server cannot observe what a client's model was given
- feat: honour a requested compact view and report when it is not applied —
  omission never enables compact output
- fix: a text-only client is pointed at the data (a named field and a size)
  instead of being handed a silently elided answer; a 256 KiB duplicate is
  replaced by a pointer, never an empty content array
- feat: `compare_nodes` compares a *fresh* read against a retained baseline
  and guards writes. A missing baseline refuses; an unreadable node is never
  silently unchanged; absent / null / false / 0 / "" stay five states; every
  response carries `atomic: false` and `exactlyOnce: false`
- fix: classify a guarded refusal by what the caller asked, not by the method
  name, and fail closed when the guard itself cannot be read

### Evidence, receipts and retention

- feat: a durable, crash-journaled operation journal; every real bridge
  dispatch records a receipt, and a client can read back its own and account
  for unobserved work. A daemon's mutation counter advances on dispatch, not
  on effect, so it is never used to decide a field is unchanged
- feat: crash-journaled evidence store with owner-scoped export; only
  `inspect` captures today
- fix: a loosened store root is refused, not silently repaired

### New tools for existing jobs

- feat: `sample_data` — exact numerical reads. Validated SOP point adapter,
  and image coordinates are refused there
- feat: `script_source` — named Python sources with immutable content refs
- feat: `param_interface` — export a control surface, apply one additively
- feat: `project_snapshot` — a save receipt naming what was written, and
  where
- fix: a CHOP's `numpyArray` is channel-major, not sample-major. The shape is
  now cross-checked against the CHOP's own `numChans`/`numSamples` and a
  mismatch is refused, because a flag on bad data is not a guard
- fix: a re-registration must not report dependencies it did not apply
- fix: a menu whose items could not be set must not read as applied
- fix: a component receipt must not carry the project's unsaved state, and
  must not claim it can — `COMP.dirty` reads False even after an edit and a
  save, so the answer is `available: false` *with a reason*

### Correctness fixes found by measurement

- fix: a project save over MCP ends the stream without a response while the
  bridge stays connected; it is now a specific refusal, and a *component*
  save is unaffected
- fix: threading issues in the bridge, and timing-aware capture/record
- fix: a slow launch is no longer reported as a stalled link, and a launch is
  budgeted by the caller's own wait rather than a tool class
- fix: `spawn_td` reports which build actually ran, and a wait timeout says
  the process is still running
- fix: a spawned TouchDesigner's ownership is kept until it exits
- fix: `spawn_td` tells the child which bridge port the daemon actually bound.
  The bridge inside TouchDesigner resolves its daemon from the environment
  only, so on a non-default `[bridge] port` it used to dial `9861` regardless —
  never connecting, or attaching a TouchDesigner to a *different* daemon that
  owned 9861. Found by live acceptance, not by a test.
- fix: that port export now honours the daemon's own `TDMCP_IPC_PORT`
  override instead of re-deriving the port from `[bridge] port` alone. A
  daemon started with `TDMCP_IPC_PORT=…` binds that port while the config
  still names another, so the first fix handed the child a stale port and
  reintroduced the same wrong-daemon attachment. Proven live, and the
  precedence is mutation-tested.
- fix: project unpack and bridge install are fail-safe under concurrent
  writes
- feat: set aside the file a replace is about to destroy
- feat: report what a fresh load would cost, and perform none of it

### Palette

- feat(palette): `palette_index` (offline roster: scan / list / get /
  describe / ignore / unignore / forget / stats) and `palette_probe` (bridged
  interface digests). Cards are **authored** from a digest; nothing generates
  one
- feat(mutate): `place` step — drop a Palette component into the network by
  `paletteId` or `toxPath`, wired in the same batch
- feat(projectio): palette root discovery on `InstallInfo` plus an
  index/card store with fingerprint staleness and id globs
- feat(config): `[palette]` section — `user_root`, `store_dir`, and a seeded
  probe blacklist. Loading a blacklisted component runs its startup scripts,
  so the default probe skips it
- feat(palette): unwrap the palette wrapper — probe digests the real
  component and lifts its `help` DAT; `place` lifts the component out of the
  icon shell via `copyOPs`
- feat(palette): `thumbnails: true` renders a 256px PNG per component; a
  black or uniform frame is *reported* rather than stored
- fix(palette): a fully blacklisted or empty selection explains itself instead
  of returning a silent empty batch
- fix(palette): an id absent from the index fails
  `tdmcp.palette.unknown_id` rather than quietly skipping it
- fix(palette): a never-dispatched call clears its breadcrumb rather than
  flagging innocent components `suspect`
- fix(palette): empty extension slots no longer report a `NoneType` class
- fix: the observe-change template is committed behind the rendered skill

### Guidance and skills

- feat: V2 guidance ships as capability-gated overlays; task guidance is
  split and classified examples are validated
- feat(gui): Palette dashboard section — browsable roster, rendered
  thumbnails, a copy-to-paste agent brief per component, and an Analyse run.
  Cards are never written from the GUI
- feat(skills): `palette` and `palette-scan` cards, and a "look in the
  Palette before hand-building" umbrella rule
- fix: correct current-tool guidance and unify task routing
- feat: targeted tool discovery and identity in the fleet roster; bridge code
  generations are identified and the v2 output contract is typed

### Platform, packaging and process

- feat(linux): dialogs and TD process lifecycle management; ksni tray
  backend; GUI tools rows and AUR install detection
- fix(dialogs): macOS dialog detection actually works
- fix(gui): popup closes on outside click, not cursor move; X11 backend
  forced on Linux
- feat(gui): logo branding, a centered sidebar identity footer, and Clear
  buttons for the ACTIVITY card and the Logs toolbar
- feat: ship verifiable per-target third-party notices; **blanket daemon
  kills are removed**
- fix: winit's crossfont CSD backend selected, completing the ttf-parser
  removal

### Not claimed

No model token A/B significance, no agent task-acceptance measurement, no
native Windows or macOS qualification from a Linux host, and **unsigned
installers**. TouchDesigner under Wine was exercised live on the build host
(handshake, inspect, create, guarded write, capture, graceful kill) and passes,
but that is a test convenience, not a support statement — the Wine path needs
a local launcher whose graphics and licensing setup is machine-specific.
`docs/MIGRATION.md` states the same list in full, and
`scripts/scorecard_report.py` prints it alongside the four measured cards.

## Legacy unreleased notes

### Added
- feat(gui): new logo across the app — README header, sidebar brand mark,
  tray-popup header, and tray/window/installer icons regenerated from
  `logo.svg` via `packaging/gen_icons.py`
- feat(gui): sidebar footer carries role badge + `v<ver> · up <t>` meta
  (moved out of the dashboard top bar); sidebar header is now the centered
  logo mark
- feat(gui): Clear buttons for the ACTIVITY card and the Logs toolbar
  (client-side view clears)
- feat(palette): palette awareness — `palette_index` (offline roster: scan / list / get / describe / ignore / unignore / forget / stats) and `palette_probe` (bridged interface digests)
- feat(mutate): `place` step — drop a Palette component into the network by `paletteId` or `toxPath`, wired in the same batch
- feat(projectio): palette root discovery on `InstallInfo` + index/card store with fingerprint staleness and id globs
- feat(config): `[palette]` section — `user_root`, `store_dir`, and a seeded probe blacklist
- feat(skills): `palette` and `palette-scan` cards; "look in the Palette before hand-building" umbrella rule
- feat(palette): unwrap the palette wrapper — probe digests the real component (custom pars, In/Out pins) and lifts its `help` DAT; `place` lifts the component out of the icon shell via `copyOPs`
- feat(palette): `palette_probe {thumbnails: true}` renders a 256px PNG per component into `{store}/thumbs/` (wrapper icon art first, shared-viewer fallback); `list`/`get` expose the stored path as `thumb`, and a black/uniform frame is reported rather than stored
- feat(gui): Palette dashboard section — browsable roster (search + card-state filters over the same selector the tools use), rendered thumbnails, a ready-to-paste "Copy reference" agent brief per component, and an Analyse run (rescan → probe → thumbnails) that ends in a handoff brief; cards are never written here — that stays the agent's job

### Fixed
- fix(palette): a fully blacklisted or empty probe selection now explains itself (`skipped` / `skippedTotal` / `note`) instead of returning a silent empty batch
- fix(palette): probing an id absent from the index fails `tdmcp.palette.unknown_id` instead of quietly skipping it
- fix(palette): only a timeout or mid-call disconnect strands the in-flight breadcrumb; a never-dispatched call clears it rather than flagging innocent components `suspect`
- fix(palette): empty extension slots no longer report as a `NoneType` class

## v0.1.4 — 2026-08-30

### Added
- Project I/O — `td_installs`, `project_unpack`, `project_pack`,
  `project_lint`, and `project_install_bridge`: offline `.toe`/`.tox` editing
  over the official `toeexpand`/`toecollapse` tools with strict filesystem
  verification, plus bridge injection (update + create-from-scratch) into any
  project file
- Lifecycle — `spawn_td` / `kill_td` with deterministic pid ownership,
  pre-handshake registration, and startup-dialog surfacing
- Dialogs — `dialogs` tool (list / describe / dismiss) backed by a
  daemon-side popup watcher: Win32 user32 + in-house UIA on Windows,
  CGWindowList + Accessibility on macOS; bridged calls fail fast with
  `tdmcp.dialog.blocking` when a modal intercepts
- Federation — master/slave daemon proxying (LAN-only, PSK-protected):
  register, fleet push, and proxied tool calls
- Claude Code — self-contained plugin with bundled skills; tag-driven
  multi-platform release pipeline (Inno Setup installer + macOS DMG)
- GUI — tray dashboard (Overview / Logs / Settings, Ableton-dark design
  system), glance popup with action footer, crash-report surfacing
- Observability — central JSONL sink with rotation + retention, bridge
  uplink, TD face-LOGS mirror, admin log endpoints, stdio-proxy ingest,
  message-hygiene audit
- Mutate — shader compile lint on DAT text writes; first-class `OP.comment`
  on `mutate_nodes` and `inspect`
- Skills — Jinja-templated TouchDesigner skill cards, served as MCP
  resources or rendered to files

### Changed
- Bridge transport migrated from OS named pipes to TCP loopback on all
  platforms (one `.tox` fits every OS)
- Docs synced to the shipped contract: v1 tools, TCP transport, v2 spec
  deviations dispositioned

### Fixed
- Windows bridge transport stabilized — framed-read offsets, disconnect
  teardown, black-frame detection (all found and fixed against live TD)
- Silent-loss and transport bugs from the live limits audit: oversized
  results truncate instead of discarding work, capture rejects oversized
  sizes pre-flight, IPC frame raised to 32 MiB, curated JSON rejections
- `spawn_td` `wait_timeout` payload correctness, dialogs hang-probe
  semantics, helper-window false positives in popup detection
- toc layout / collapse staging / scan-root dedup across project I/O
- stdio-proxy error forwarding and response-shape consistency; fuzzy
  did-you-mean suggestions on tool arguments
- macOS compatibility + MSRV gates; Linux CI gates (GTK deps, EOL-normalized
  tox drift hash)

