# Concurrency fuses — baseline inventory

Suite: `crates/tdmcp-daemon/tests/concurrency_fuses.rs`  
Command:

```text
cargo test -p tdmcp-daemon --test concurrency_fuses -- --nocapture --test-threads=1
```

**Session policy:** create suite + inventory only — **no production `src/` fixes**.

## Double-run (post harness Notify fix)

Harness note: callers must `let w = held.notified(); pin!(w);` **before** spawning
work that notifies. An earlier draft awaited `held.notified()` after spawn and
missed fast notifies → outer TIMEOUT on H1/H2/X2 (harness bug, not daemon).

| ID | Test | Run 1 | Run 2 | Verdict |
| --- | --- | --- | --- | --- |
| E1 | `easy_two_shared_same_pid_ok` | PASS | PASS | PASS |
| E2 | `easy_exclusive_rejects_while_shared_held` | PASS | PASS | PASS |
| E3 | `easy_two_pids_concurrent_ok` | PASS | PASS | PASS |
| M1 | `med_shared_storm_fifo_echo` | PASS | PASS | PASS |
| M2 | `med_exclusive_storm_while_held` | PASS | PASS | PASS |
| M3 | `med_pid_loss_isolates_peer` | PASS | PASS | PASS |
| H1 | `hard_saturate_job_channel_then_drain` | PASS | PASS | PASS |
| H2 | `hard_saturate_then_disconnect_flushes` | PASS | PASS | PASS |
| H3 | `hard_supersede_while_inflight_held` | PASS | PASS | PASS |
| X1 | `x_saturate_then_supersede` | PASS | PASS | PASS |
| X2 | `x_asymmetric_storm_two_pids` | PASS | PASS | PASS |

Summary: **11/11 PASS** both runs — no FLAKE, no solid production FAIL.

Wall times: run1 ≈ 0.19s tests / run2 ≈ 0.14s tests (after compile).

## Pre-fix probe (harness only — not a production finding)

First execution before subscribe-before-spawn fix:

| ID | Outcome | Excerpt | Layer |
| --- | --- | --- | --- |
| H1 | TIMEOUT | `test exceeded outer timeout 10s` | `harness` (Notify miss) |
| H2 | TIMEOUT | `test exceeded outer timeout 10s` | `harness` (Notify miss) |
| X2 | TIMEOUT | `test exceeded outer timeout 15s` | `harness` (Notify miss) |
| E\* / M\* / H3 / X1 | PASS | — | — |

Those three are **not** carried as daemon defects for the fix session unless
they regress after the Notify harness pattern.

## Fix-session handoff

No production candidates from this baseline. Optional follow-ups:

- Stress larger `K` / longer hold under load if desired
- Deferred Extreme: heartbeat × tool `select!`, exclusive during supersede, three-pid fanout
- Keep Notify subscribe-before-spawn as harness law when adding cases
