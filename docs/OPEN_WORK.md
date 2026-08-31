# Open work

The single honest list of what is not done yet. Big items have their own plan
file; small ones are fully specified here.

## In flight — Linux / Wine support

Full spec + phases + live gates: [`LINUX_SUPPORT.md`](LINUX_SUPPORT.md).

Shipped: the TCP-transport migration for all OSes (P0/P1/P1b — named pipes are
gone, the bridge speaks TCP loopback everywhere). Open:

- **P2 — Linux lifecycle.** `spawn_td` / `kill_td` under Wine, plus one
  follow-up parked from P0 (move the self-probe respawn watchdog into
  `daemon_link.rs`'s healthy branch).
- **P3 — Unsupported surfacing.** What a user sees/gets on an unsupported
  platform (docs, diagnostics, honest degrade). GUI-stack failures already
  degrade to headless serving and the tray is `ksni` (no GTK dependency);
  the `dialogs`-on-Linux coded error (L-8) still lands here.
- **P4 — CI + packaging + docs.** Linux build in CI, packaging, doc updates
  (CONTRACT / CONFIG / DELIVERY amendments).
- **P5 — Live E2E under Wine** (needs the user: gates G-L2–G-L4).

## Planned — bounded payloads + artifact spool

Full design + phases: [`PAYLOAD_SPOOL_PLAN.md`](PAYLOAD_SPOOL_PLAN.md).
Nothing implemented yet. Phase 0 (bound `inspect` DAT content with
`tdmcp.op.content_truncated`) is the correctness fix; phases 1–3 add the
artifact spool, HTTP serving + federation rule, and `format` / result spill.

## Deferred — limits residue

Leftovers from the limits audit (all smaller fixes are landed):

1. **Proxy ceilings → config.** `TDMCP_PROXY_*_TIMEOUT_MS` env-only knobs
   (`crates/tdmcp-mcp/src/daemon_link.rs`) should become `[proxy]` config
   keys with docs and back-compat (CONFIG.md addition).
2. **Fleet pump-staleness visibility.** `fleet` only says
   `connected`/`disconnected`; an agent cannot tell "busy, still alive" from
   "about to time out". Add last-heartbeat age / TD main-thread pump
   staleness. Touches the live bridge (`tox_callbacks.py` pump timestamp) →
   requires `scripts/pack_bootstrap_tox.md` re-run + live-instance reload.
3. **Bridge timeout config defaults.** Raising
   `call_timeout_secs`/`script_timeout_secs`/`heartbeat`/`pong`/`idle_dead`
   defaults (target 600 s script timeout) is fully specified but blocked on a
   live soak test — a long-tail call was only verified live up to ~70 s;
   shipping an unverified number is exactly the class of bug this work
   exists to avoid.
4. **Body-limit rejection envelope (partial).** Curated JSON rejection on
   oversized bodies landed on `/mcp/tools/call` only; federation/admin routes
   still return axum's raw `413`. Only daemon-to-daemon / GUI traffic hits
   those today — low priority.
