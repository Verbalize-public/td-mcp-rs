# Testing strategy

Local-first. No CI planned yet — run `scripts/check.ps1` / `scripts/check.sh`
before declaring a gate green.

## Layers

| Layer | Location | Live TD? |
| --- | --- | --- |
| Unit | each crate `src/**` / `tests/` | no |
| Integration | `crates/tdmcp-daemon/tests/` + `tdmcp-test-support` | no |
| Bridge pytest | `bridge/tests/` (fake `td` module) | no |
| Dev env (interactive) | [`DEV_ENV.md`](DEV_ENV.md) + [`fixtures/dev/`](../fixtures/dev/) | yes |
| Manual E2E | [`E2E_CHECKLIST.md`](E2E_CHECKLIST.md) | yes |

## Deep tests (must stay green)

1. Exclusive queue fails when any shared task is queued.
2. Resurrection stack persists when first post-reconnect task fails; clears only on success.
3. Pid-reuse fingerprint mismatch clears that pid's state only.
4. OpPath similar-name lint (bounded) — when implemented (P1).
5. `mutate_nodes` sequential apply — stops at first hard failure (`failedAt`); later steps emit `tdmcp.batch.skipped_dependent`; pure `apply_step` seam unit-covered without TD (`bridge/tests/test_mutate.py` + `mcp_tools.rs`).
6. Diagnostics catalog completeness — every emitted `code` has a catalog entry.

## Running

```text
cargo test --workspace
scripts/check.ps1
```

Integration tests spin a fake TD peer via `tdmcp-test-support` speaking the real
IPC wire protocol — they must not require TouchDesigner installed.
