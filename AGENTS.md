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

### Hard Rules

- **MCP-first** for live claims — never claim success from code alone.
- **pid only** — never invent sticky targets.
- **Never-panic** — no `unwrap`/`expect`/`panic!` in lib release paths without `RISKS.md`.
- **Stop after 3 failed probes** with no new evidence.

### Documentation Reference

| Need | Go to |
| --- | --- |
| v1 contract / tools / OpPath | [`docs/CONTRACT.md`](docs/CONTRACT.md) |
| Install / quickstart | [`README.md`](README.md) |
| Crate boundaries / topology | [`ARCHITECTURE.md`](ARCHITECTURE.md) |
| Never-panic / lints / DRY | [`CONSTITUTION.md`](CONSTITUTION.md) |
| Accepted panic/unsafe exceptions | [`RISKS.md`](RISKS.md) |
| Local quality gate | `scripts/check.ps1` / `scripts/check.sh` |
| Config file | [`docs/CONFIG.md`](docs/CONFIG.md) |
| Packaging | [`docs/DELIVERY.md`](docs/DELIVERY.md) |
| Claude Code plugin (`.claude-plugin/`, `claude-skills/`) | [`docs/CLAUDE_CODE_PLUGIN.md`](docs/CLAUDE_CODE_PLUGIN.md) |
| Logging / central sink / observability | [`docs/OBSERVABILITY.md`](docs/OBSERVABILITY.md) (spec) + [`docs/OBSERVABILITY_PLAN.md`](docs/OBSERVABILITY_PLAN.md) (execution plan) |
| Tool-call arg-error diagnostics (`tdmcp.args.*`) | [`docs/TOOL_ERROR_PLAN.md`](docs/TOOL_ERROR_PLAN.md) |
| GUI overhaul plan (active work spec) | [`docs/GUI_OVERHAUL_PLAN.md`](docs/GUI_OVERHAUL_PLAN.md) |
| Payload limits audit / artifact-spool plan (base64 → file delivery) | [`docs/LIMITS_AUDIT.md`](docs/LIMITS_AUDIT.md) + [`docs/PAYLOAD_SPOOL_PLAN.md`](docs/PAYLOAD_SPOOL_PLAN.md) |
| **Start here after a handoff** | [`docs/HANDOFF_V2.md`](docs/HANDOFF_V2.md) |
| v2 tools spec (project I/O, lifecycle, dialogs) | [`docs/SKILLS_CONTRACT_PROPOSAL.md`](docs/SKILLS_CONTRACT_PROPOSAL.md) + [`docs/V2_IMPLEMENTATION_PLAN.md`](docs/V2_IMPLEMENTATION_PLAN.md) |
| Dialogs design / dismiss ladder | [`docs/DIALOGS.md`](docs/DIALOGS.md) |
| Agent skill cards (authoring contract) | [`skills/README.md`](skills/README.md) |
| Test layout / what runs where | [`docs/TESTING.md`](docs/TESTING.md) |
| E2E acceptance rows | [`docs/E2E_CHECKLIST.md`](docs/E2E_CHECKLIST.md) |

### Build / check

```text
cargo build --workspace
scripts/check.ps1   # Windows
scripts/check.sh    # Unix
```

After `src/` / schema / catalog changes: rebuild daemon, restart MCP client if needed.
