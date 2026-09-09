# Play state (timeline / cooking)

When TD looks "stuck" or captures look frozen, check transport **before**
rewriting the network.

## Agent rules

1. Project **play/pause** gates cooking: paused means most cooks (and CHOP
   time-slicing) stall.
2. `inspect` / `capture` then reflect the frame at pause time — not a live
   signal. Do not treat a paused black/static frame as a network FAIL until you
   have confirmed play is on (or the claim is specifically about the paused
   frame).
3. The bridge pump should remain responsive while paused. Do not resume a
   controlled timing job just to service tools: use its status/cancel surface.
   If the main thread is genuinely blocked, report the timeout; an RPC timeout
   does not establish that the job was cancelled.

## Quick checks

- For tool-managed sampling/recording and transport actions, use
  [`timed-capture`](./timed-capture.md). Ordinary inspection never changes play state.
- For repeatable stateful checks, use the public reset signal and advance
  sequentially with verified cooking ([`reset-state`](./reset-state.md)). Timeline
  seeking or moving backward does not reset the network's accumulated state.
- Timeline / Perform transport: is the project playing?
- After unpausing, re-`inspect` / re-`capture` before grading look or FPS.
- Sequential bridged calls still apply: [`tooling-concurrency`](./tooling-concurrency.md).

## Related

- Look grading: [`look-grade`](./look-grade.md)
- Structural DoD: [`definition-of-done`](./definition-of-done.md)
- Cook model depth: [`primer/cook-and-families`](../primer/cook-and-families.md)

---

**Canonical:** [`play-state`](./play-state.md)