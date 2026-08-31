# Testing strategy

Local-first with CI: run `scripts/check.ps1` / `scripts/check.sh` before
declaring a gate green; CI runs fmt/clippy/test on every push and the release
workflow builds installers on `v*` tags (see [`DELIVERY.md`](DELIVERY.md)).

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
4. OpPath similar-name lints (`tdmcp.par.similar_name`, `tdmcp.op.similar_type`) — best-effort did-you-mean hints attached to hard failures, never redirecting the outcome (`mutate_forwards_similar_name_and_similar_type_lints`, `args_diag.rs` similar-field test, `error_surface.rs`).
5. `mutate_nodes` sequential apply — stops at first hard failure (`failedAt`); later steps emit `tdmcp.batch.skipped_dependent`; pure `apply_step` seam unit-covered without TD (`bridge/tests/test_mutate.py` + `mcp_tools.rs`).
6. Diagnostics catalog completeness — every emitted `code` has a catalog entry.
6b. `api_help` — live API cards / classes index / thin module; soft-cap 32 queries; partial entry failure; FakeTdPeer round-trip (`bridge/tests/test_api_help.py` + `bridge_session.rs` + schema golden).
7. Stdio proxy reconnect — kill HTTP daemon mid-session → `tdmcp.daemon.unreachable`; restart on same port → subsequent `fleet` succeeds without relaunching the stdio process; watcher heals without an intervening successful tool call (`crates/tdmcp-daemon/tests/stdio_proxy.rs`).
8. **Concurrency fuse ladder** (no live TD) — `crates/tdmcp-daemon/tests/concurrency_fuses.rs`:
   - Easy: two shared same pid; exclusive rejects while shared held; two pids concurrent OK
   - Medium: shared storm FIFO echo (`K=8`); exclusive storm while held; pid-loss isolates peer
   - Hard: saturate actor `JOB_CHANNEL_CAPACITY` then drain; saturate then disconnect flushes; supersede while in-flight held
   - Extreme: saturate then supersede; asymmetric A-saturate / B-storm
   - Reproducibility: `Notify` phase barriers (not sleep-as-sync); poll-with-budget; outer + caller timeouts; Medium+ use `multi_thread` (4 workers).
9. **Multi-client transport freeze** (no live TD) — `crates/tdmcp-daemon/tests/multi_client_freeze.rs`:
   - Spawns a real `tdmcp-daemon` binary (no GUI) on a free port with fake TD peers
     over the real TCP bridge transport, then drives it with several concurrent rmcp Streamable
     HTTP sessions firing bursts of `fleet` / `inspect` / `execute_python` calls,
     session churn, and mid-call session teardown.
   - Asserts: a probe session's `fleet` never misses its budget, raw TCP connects and
     fresh-session `fleet` still work after the storm, and the number of `TIME_WAIT`
     sockets on the daemon port stays bounded (< 2000).
   - **Root cause this guards against:** rmcp's default Streamable HTTP client sets
     `pool_max_idle_per_host(0)` (no connection reuse), so every tool call opens a
     fresh TCP connection; each closed connection parks in `TIME_WAIT` for minutes
     (the OS dynamic port range is finite — Windows 49152–65535 is the canonical
     example), exhausting it under
     sustained multi-client load. `connect()` then fails with `WSAEADDRINUSE` and the
     MCP transport looks completely frozen while the daemon itself is healthy. The
     stdio proxy supplies its own bounded idle pool instead
     (`tdmcp_mcp::daemon_link::connect_http`).
10. **Wedged-session rescue** (no live TD) — `crates/tdmcp-daemon/tests/stdio_proxy.rs`
    (`stdio_proxy_call_timeout_heals_and_returns_budget_error`):
    - A bridged call the daemon never answers (fake bridge gate held) must not hang
      the MCP client forever: the stdio proxy bounds every forwarded call (defaults
      above the `[bridge]` budgets, env-tunable via `TDMCP_PROXY_*_TIMEOUT_MS`), and
      on budget expiry heals the link (fresh session) and returns
      `tdmcp.daemon.unreachable` with `budgetMs`. A follow-up bridged call on the
      healed link then succeeds.
    - This is the recovery half of the SSE-backpressure hazard: rmcp's per-session
      worker can block on a full SSE stream when a client stops reading, and new
      requests from that session pile up behind it with no server-side timeout. The
      proxy timeout converts that indefinite hang into a bounded error + heal; the
      heal's disconnect also unwedges the daemon-side session.

11. **Palette awareness** (no live TD) — three seams, none of which may touch the
    developer's real palette:
    - `tdmcp-projectio/src/palette.rs` — root walk, id derivation, scan
      add/update/remove, source-scoped pruning, fingerprint staleness, ignore
      globs, store round-trip, and a corrupt/versioned `index.json` degrading to
      `tdmcp.palette.store_failed` instead of panicking.
    - `tdmcp-mcp/src/palette.rs` — every action, selector filtering and paging,
      `describe` demanding a summary, `forget` refusing to wipe without a
      selector, the in-flight breadcrumb becoming `suspect`, probe batch caps,
      and auto-ignore at the second strike. Install discovery is **injected**
      (`NO_INSTALLS`), because `default_scan_roots` hardcodes `/Applications` on
      macOS and the host's real 281-component palette would otherwise leak into
      every assertion.
    - `bridge/tests/test_palette_probe.py` + `test_mutate.py` — digest shaping
      against fake COMPs (including one whose every property raises, since a
      probe runs arbitrary third-party code), scratch-COMP teardown even when a
      load raises, one bad component staying one bad row, and `place`
      rename-lint / rollback / alias remap.
    Live rows P1–P10 in [`E2E_CHECKLIST.md`](E2E_CHECKLIST.md) — last verified
    PASS 2026-08-31.

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
