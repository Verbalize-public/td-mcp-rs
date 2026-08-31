# Changelog

## Unreleased

### Added
- feat(palette): palette awareness — `palette_index` (offline roster: scan / list / get / describe / ignore / unignore / forget / stats) and `palette_probe` (bridged interface digests)
- feat(mutate): `place` step — drop a Palette component into the network by `paletteId` or `toxPath`, wired in the same batch
- feat(projectio): palette root discovery on `InstallInfo` + index/card store with fingerprint staleness and id globs
- feat(config): `[palette]` section — `user_root`, `store_dir`, and a seeded probe blacklist
- feat(skills): `palette` and `palette-scan` cards; "look in the Palette before hand-building" umbrella rule
- feat(palette): unwrap the palette wrapper — probe digests the real component (custom pars, In/Out pins) and lifts its `help` DAT; `place` lifts the component out of the icon shell via `copyOPs`

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

