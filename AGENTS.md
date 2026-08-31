# AGENTS.md — td-mcp-rs

## Quick Start

This project uses `/understand-chat` for codebase context. Run `/understand` first, then ask questions.

### Development Workflow

1. **Install binary:** `tdmcp-daemon ensure`
2. **Build:** `cargo build --workspace`
3. **Check:** `cargo clippy --workspace --all-targets -- -D warnings`
4. **Test:** `cargo test --workspace`

### MCP Restart (Critical!)

When changing binary code:

1. **Kill all running mcp daemons** (blindly):
   ```powershell
   # Windows
   taskkill /IM tdmcp-daemon.exe /F
   ```
   ```bash
   # Unix
   pkill -f tdmcp-daemon
   ```

2. **Then rebuild:** `cargo build --workspace`

3. **Then install:** `tdmcp-daemon ensure`

**Stop after 3 failures or handoff to user.** The daemon installed binary may be used by the harness or running standalone, causing file locks.

### Editing `bridge/bootstrap.py` or `bridge/tox_callbacks.py`?

These two files get baked into `crates/tdmcp-daemon/embedded/bootstrap.tox` —
an opaque TD binary format nothing outside TD can read or patch. Editing
either `.py` file without repacking silently ships a stale tox with **no
error anywhere** unless `cargo test` catches it
(`bootstrap_tox_matches_packed_source_hash` in
`crates/tdmcp-daemon/src/install.rs`). If that test goes red, or you touched
either file: read [`scripts/pack_bootstrap_tox.md`](scripts/pack_bootstrap_tox.md)
first — it explains the four copies this one `.tox` propagates through and
exactly which command re-syncs each one, including `cargo run -p xtask --
stamp-tox` to close the drift check. Do not silence or delete that test to
make it pass.

### Editing `skills/` (templates or `MANIFEST.yaml`)?

`claude-skills/` is a **checked-in render** of `skills/`, and it is what the
Claude Code plugin actually ships to users — a plugin install is a plain git
checkout, so nothing regenerates it at install time. Editing a `.jinja.md`
card or `MANIFEST.yaml` without re-rendering ships stale skills to every
plugin user while the repo source looks correct. Always finish the edit with:

```text
cargo run -p tdmcp-daemon -- skills render --dest claude-skills
git add claude-skills
```

`claude_plugin_skills_match_rendered_output` (in
`crates/tdmcp-daemon/src/install.rs`) fails the build on any byte-for-byte
drift. Do not silence or delete that test to make it pass. Authoring contract:
[`skills/README.md`](skills/README.md); plugin layout:
[`docs/CLAUDE_CODE_PLUGIN.md`](docs/CLAUDE_CODE_PLUGIN.md).

### Hard Rules

- **MCP-first** for live claims — never claim success from code alone.
- **pid only** — never invent sticky targets.
- **Never-panic** — no `unwrap`/`expect`/`panic!` in lib release paths without `RISKS.md`.
- **Stop after 3 failed probes** with no new evidence.
- **Re-render `claude-skills/`** in the same commit as any `skills/` edit —
  `cargo run -p tdmcp-daemon -- skills render --dest claude-skills`.

### Documentation Reference

| Need | Go to |
| --- | --- |
| Contract of record (tools, shapes, diagnostics) | [`docs/CONTRACT.md`](docs/CONTRACT.md) |
| Install / quickstart | [`README.md`](README.md) |
| Crate boundaries / topology | [`ARCHITECTURE.md`](ARCHITECTURE.md) |
| Never-panic / lints / DRY | [`CONSTITUTION.md`](CONSTITUTION.md) |
| Accepted panic/unsafe exceptions | [`RISKS.md`](RISKS.md) |
| Local quality gate | `scripts/check.ps1` / `scripts/check.sh` |
| Config file | [`docs/CONFIG.md`](docs/CONFIG.md) |
| Packaging | [`docs/DELIVERY.md`](docs/DELIVERY.md) |
| Claude Code plugin (`.claude-plugin/`, `claude-skills/`) | [`docs/CLAUDE_CODE_PLUGIN.md`](docs/CLAUDE_CODE_PLUGIN.md) |
| Logging / observability (spec; only `td.errors` polling deferred) | [`docs/OBSERVABILITY.md`](docs/OBSERVABILITY.md) |
| GUI internals / current map | [`docs/GUI_MAP.md`](docs/GUI_MAP.md) |
| **What is not done yet** (Linux/Wine · payload spool · limits residue) | [`docs/OPEN_WORK.md`](docs/OPEN_WORK.md) → [`docs/LINUX_SUPPORT.md`](docs/LINUX_SUPPORT.md) · [`docs/PAYLOAD_SPOOL_PLAN.md`](docs/PAYLOAD_SPOOL_PLAN.md) |
| Day-to-day live-TD dev harness | [`docs/DEV_ENV.md`](docs/DEV_ENV.md) |
| **Start here after a handoff** | [`docs/CONTRACT.md`](docs/CONTRACT.md) + [`docs/OPEN_WORK.md`](docs/OPEN_WORK.md) |
| Agent skill cards (authoring contract) | [`skills/README.md`](skills/README.md) |
| Test layout / what runs where | [`docs/TESTING.md`](docs/TESTING.md) |
| Palette awareness (tools, store, blacklist) | [`docs/CONTRACT.md`](docs/CONTRACT.md) § `palette_index` / `palette_probe` |
| E2E acceptance rows | [`docs/E2E_CHECKLIST.md`](docs/E2E_CHECKLIST.md) |

### Build / check

```text
cargo build --workspace
scripts/check.ps1   # Windows
scripts/check.sh    # Unix
```

After `src/` / schema / catalog changes: rebuild daemon, restart MCP client if needed.
