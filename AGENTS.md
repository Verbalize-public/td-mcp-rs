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
| Logging / central sink / observability | [`docs/OBSERVABILITY.md`](docs/OBSERVABILITY.md) (spec) + [`docs/OBSERVABILITY_PLAN.md`](docs/OBSERVABILITY_PLAN.md) (execution plan) |

### Build / check

```text
cargo build --workspace
scripts/check.ps1   # Windows
scripts/check.sh    # Unix
```

After `src/` / schema / catalog changes: rebuild daemon, restart MCP client if needed.
