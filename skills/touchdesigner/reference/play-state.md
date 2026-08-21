# Play state (timeline / cooking)

When TD looks "stuck" or captures look frozen, check transport **before**
rewriting the network.

**Canonical:** `tdmcp://docs/play-state` 

## Agent rules

1. Project **play/pause** gates cooking: paused means most cooks (and CHOP
   time-slicing) stall.
2. `inspect` / `capture` then reflect the frame at pause time — not a live
   signal. Do not treat a paused black/static frame as a network FAIL until you
   have confirmed play is on (or the claim is specifically about the paused
   frame).
3. If bridged tools time out while the timeline is paused, press **Play** and
   retry once. Prefer fixing play state over restarting the daemon or tox.

## Quick checks

- Timeline / Perform transport: is the project playing?
- After unpausing, re-`inspect` / re-`capture` before grading look or FPS.
- Sequential bridged calls still apply: `tdmcp://docs/tooling-concurrency`.

## Related

- Look grading: `tdmcp://docs/look-grade`
- Structural DoD: `tdmcp://docs/definition-of-done`
- Cook model depth: `tdmcp://docs/primer/cook-and-families`
