# Claude Code plugin

td-mcp-rs ships a self-contained [Claude Code](https://claude.com/claude-code)
plugin from this same repo — no separate marketplace repo, no hand-edited MCP
JSON. It adds a first-class install path alongside the existing
[`mcp.tdmcp.example.json`](../mcp.tdmcp.example.json) flow that Cursor (and
any other MCP-compatible harness) keeps using unchanged.

## Layout

| Path | Role |
| --- | --- |
| [`.claude-plugin/plugin.json`](../.claude-plugin/plugin.json) | Plugin manifest — name, `userConfig` (daemon path), pointers to the two pieces below |
| [`mcp-servers.json`](../mcp-servers.json) | The `tdmcp-rs` MCP server entry, referenced from `plugin.json` |
| [`claude-skills/`](../claude-skills/) | Checked-in, filesystem-mode render of `skills/` — one `SKILL.md` per Claude Code's own format |

`plugin.json` must live at the repo root for the one-line
`/plugin marketplace add <owner>/<repo>` install to work (Claude Code infers
a single-plugin marketplace from a root `plugin.json`; it has no documented
way to point a marketplace entry at a subdirectory of a shared repo). The
existing top-level `skills/` directory (Jinja templates + `MANIFEST.yaml`,
consumed by `tdmcp-mcp` / `tdmcp-daemon skills render`) is left untouched;
`plugin.json`'s `skills` field points at `./claude-skills/` explicitly instead
of relying on the default `./skills/` scan, so the two never collide.

## Why `mcp-servers.json`, not `.mcp.json`

Claude Code auto-loads a project-root **`.mcp.json`** as a plain project MCP
config any time this repo is opened directly — independent of the plugin
system. Our server command depends on `${user_config.daemon_path}` (see
below), which only resolves inside a plugin's own config context. Naming the
file `mcp-servers.json` (referenced from `plugin.json` via
`"mcpServers": "./mcp-servers.json"`) keeps it out of that auto-load path, so
opening the repo directly in Claude Code never surfaces a broken MCP server
entry with an unresolved placeholder.

## Why `userConfig` instead of a path-guessing wrapper script

`tdmcp-daemon` is never bundled inside the plugin — it's the same binary the
user already built or installed per the root [`README.md`](../README.md), and
its install location genuinely varies (`{data_dir}/bin/` for the dev flow,
`.../Programs/tdmcp-rs/` or `/Applications/tdmcp.app/...` for the packaged
installers — see [`DELIVERY.md`](DELIVERY.md)), with no PATH registration on
either OS today. Claude Code's `.mcp.json`/plugin format has no documented way
to branch a single `command` value by OS, and Windows `.cmd`/`.bat` wrapper
behavior as an MCP `command` is undocumented — so rather than gamble on
either, `plugin.json` declares a `userConfig` field of type `file`
(`daemon_path`, default `"tdmcp-daemon"`) and `mcp-servers.json` references it
as `${user_config.daemon_path}`. Claude Code prompts for it once at install
time (a native file picker) with the default already covering anyone who put
the binary on `PATH`. This is the one part of the setup that isn't fully
zero-config; if a future release adds PATH registration to `tdmcp-daemon
install`, the default alone would cover everyone and this doc should be
updated to say so.

## Why `claude-skills/` is checked in, not generated at install time

A plugin install is a plain git checkout — Claude Code never runs `cargo
build` or any other step for it. The Jinja-templated skill pack
(`skills/MANIFEST.yaml` + `skills/templates/`) already renders to this exact
shape via `tdmcp-daemon skills render --dest <dir>` (filesystem mode: relative
Markdown links, no `tdmcp://` URIs — see [`skills/README.md`](../skills/README.md)),
so `claude-skills/` is just that render, committed.

That makes it a derived artifact that can silently drift from its source,
same shape of problem as `bootstrap.tox` vs. `bridge/*.py`
(see [`AGENTS.md`](../AGENTS.md)). It's guarded the same way: the
`claude_plugin_skills_match_rendered_output` test in
`crates/tdmcp-daemon/src/install.rs` re-renders to a temp dir and fails the
build if it doesn't byte-for-byte match `claude-skills/`.

**After editing any skill card or `MANIFEST.yaml`:**

```text
cargo run -p tdmcp-daemon -- skills render --dest claude-skills
git add claude-skills
```

then `cargo test -p tdmcp-daemon` (or the normal quality gate) will pass again.

## Installing

```text
/plugin marketplace add Verbalize-public/td-mcp-rs
/plugin install td-mcp-rs@td-mcp-rs
```

Claude Code prompts once for the `tdmcp-daemon` binary path (leave the
default if it's on `PATH`). After that, the `tdmcp-rs` MCP server and the
`touchdesigner` skill are both active automatically — the skill's own
description (`MUST READ before any TouchDesigner work: ...`) is what gets it
picked up proactively, so there's no separate persona doc to keep in sync.
