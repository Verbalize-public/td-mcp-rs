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
6b. `api_help` — live API cards / classes index / thin module; soft-cap 32 queries; partial entry failure; FakeTdPeer round-trip (`bridge/tests/test_api_help.py` + `bridge_session.rs` + schema golden).
7. Stdio proxy reconnect — kill HTTP daemon mid-session → `tdmcp.daemon.unreachable`; restart on same port → subsequent `fleet` succeeds without relaunching the stdio process; watcher heals without an intervening successful tool call (`crates/tdmcp-daemon/tests/stdio_proxy.rs`).
8. **Concurrency fuse ladder** (no live TD) — `crates/tdmcp-daemon/tests/concurrency_fuses.rs`:
   - Easy: two shared same pid; exclusive rejects while shared held; two pids concurrent OK
   - Medium: shared storm FIFO echo (`K=8`); exclusive storm while held; pid-loss isolates peer
   - Hard: saturate actor `JOB_CHANNEL_CAPACITY` then drain; saturate then disconnect flushes; supersede while in-flight held
   - Extreme: saturate then supersede; asymmetric A-saturate / B-storm
   - Reproducibility: `Notify` phase barriers (not sleep-as-sync); poll-with-budget; outer + caller timeouts; Medium+ use `multi_thread` (4 workers). Double-run baseline: [`CONCURRENCY_FUSES_BASELINE.md`](CONCURRENCY_FUSES_BASELINE.md).

## Running

```text
cargo test --workspace
scripts/check.ps1
```

Concurrency fuses only:

```text
cargo test -p tdmcp-daemon --test concurrency_fuses -- --nocapture --test-threads=1
```

Integration tests spin a fake TD peer via `tdmcp-test-support` speaking the real
IPC wire protocol — they must not require TouchDesigner installed.
