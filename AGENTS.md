# AGENTS.md — td-mcp-rs

Agent entry for the Rust TouchDesigner MCP daemon. Design contract:
[`docs/CONTRACT.md`](docs/CONTRACT.md). Layout: [`ARCHITECTURE.md`](ARCHITECTURE.md).
Law: [`CONSTITUTION.md`](CONSTITUTION.md). User install/quickstart: [`README.md`](README.md).

## Route first

| Need | Go to |
| --- | --- |
| v1 contract / tools / OpPath / diagnostics | [`docs/CONTRACT.md`](docs/CONTRACT.md) |
| Install / quickstart | [`README.md`](README.md) |
| Crate boundaries / topology | [`ARCHITECTURE.md`](ARCHITECTURE.md) |
| Never-panic / lints / DRY | [`CONSTITUTION.md`](CONSTITUTION.md) |
| Accepted panic/unsafe exceptions | [`RISKS.md`](RISKS.md) |
| Local quality gate | `scripts/check.ps1` or `scripts/check.sh` |
| Live TD verify | [`docs/E2E_CHECKLIST.md`](docs/E2E_CHECKLIST.md) |
| Testing strategy | [`docs/TESTING.md`](docs/TESTING.md) |
| Packaging | [`docs/DELIVERY.md`](docs/DELIVERY.md) |
| Typing / schema policy | [`TODO_ENFORCE_TYPE.md`](TODO_ENFORCE_TYPE.md) |
| Operate skill (after P0 green) | creative-operator `cop-*` — **do not update until P0 exits green** |

## Agent ops (when driving live TD via this daemon)

1. Prefer relying on Cursor's `tdmcp-daemon mcp` upsert, or run
   `tdmcp-daemon ensure` before probing. Health URL
   (`http://127.0.0.1:9860/mcp/health`) still valid. Stdio is only the MCP
   client shim — the HTTP daemon stays up across MCP restarts.
2. `fleet` → pick connected `pid` → `inspect` → mutate → `inspect` errors →
   `capture` (perception) → perception-critic for look claims.
3. Pass `pid` every process-scoped call. Use `contextPath` for relative
   `OpPath`s (default base = `/project1`).
4. On failure, read **`diagnostics`** (codes, lints, mitigation) — not raw
   strings alone.
5. Prefer `detailLevel: summary` / `diagnosticLevel: summary`; store-first for
   `capture`.

## Operate vs Document

| Mode | Examples |
| --- | --- |
| **Operate** | Implement crates, bridge Python, run check scripts, live TD checklist |
| **Document** | Edit CONTRACT / README, CONSTITUTION, ARCHITECTURE, catalog YAML, skills |

## Hard rules

1. **MCP-first** for live claims — never claim success from code alone.
2. Do not invent sticky targets or `targetId` — **`pid` only**.
3. Do not update `cop-*` skills until Gate P0 exit green.
4. Never-panic: no `unwrap`/`expect`/`panic!` in lib release paths without
   `RISKS.md`.
5. Stop after 3 failed probes with no new evidence.

## Build / check

```text
cargo build --workspace
scripts/check.ps1   # Windows
scripts/check.sh    # Unix
```

After `src/` / schema / catalog changes: rebuild daemon, restart MCP client if
needed (HTTP daemon survives MCP restart if already running; stdio proxy is
respawned per MCP session).
