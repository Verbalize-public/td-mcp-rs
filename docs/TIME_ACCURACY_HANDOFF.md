# Timing implementation handoff

User approved implementation of `docs/PLAN_TIME_ACCURACY.md` on 2026-09-09.

- Preserve all existing dirty changes: native Textport main-thread deferral,
  pause-safe pump fix, regression tests, reset-contract skills and generated
  Claude skills. They passed live MCP checks, 358 bridge tests, workspace
  build/tests and Clippy before this implementation started.
- Reset plus sequential forward advancement is the contract. No backward seek
  or frame jumps to reconstruct state. Reset initialization/warmup is explicit;
  sample offset zero follows it. N video frames means offsets 0 through N−1.
- Live TD 2025.32460 on Wine/Linux proved one forward frame assignment followed
  by a full independent callback iteration works for counter/fade/feedback.
  A same-call assignment loop does not cook intervening states. Use
  `td.op.TDResources` for pause-independent scheduling, from main thread only.
- Movie File Out stop-frame qtrle worked. Yield after final Add Frame before
  record-off: immediate closure loses the last frame. `writeCount` is not an
  encoded-frame count. Verify finalized artifacts, including first/last pixels.
- Examples: `scripts/timing_probe/`; real MCP client:
  `scripts/mcp_probe.py`. Historical evidence: `/tmp/tdmcp-timing-ewhgIN/`.
  Rediscover PIDs before use. PID 328851 was the owned scratch; PID 95144 is
  the user's unrelated project and must not be modified. The implementation
  recreated then removed its owned probe network; original playing/realtime
  flags are restored. Downloaded evidence remains.
- Implemented shared deferred runner, timing metadata for inspect, optional
  capture actions/schedules + paired inspection, record start/status/cancel
  with job lifetime ownership and bounded artifact retrieval. Preserve plain
  capture and inspect compatibility. No audio in initial recording scope.
- Acceptance passed for reset/fade/feedback replay, paired inspection, decoded
  seven-frame video, local time, interference, source/encoder loss, cancellation,
  deadline, cross-session ownership and paused/non-realtime daemon reconnect.
  Range/finalization races have unit coverage. Federation record/chunk routing
  has fake-peer integration coverage, not a real two-host recording test.
- Native Movie File Out must be a sibling of the source (cross-COMP wires can
  silently disappear), initialized before sampling, and finalized asynchronously.
  The runner waits for the MOV header then validates codec/dimensions/FPS/count.
- Reload cleanup uses an interpreter-local runtime anchor: bootstrap removes
  sys.modules entries, so old td.run callables otherwise outlive job globals.
  The reconnect watchdog now uses wallTime=True as well as TDResources.
  Updated callbacks were genuinely packed inside TD, stamped and installed.
  Existing user projects still need the fresh bootstrap.tox dragged in again.
- Binary changes: stop daemons, build, ensure/install, reconnect MCP; stop
  after three failures without new evidence. Bootstrap/tox callback changes
  require repacking opaque bootstrap.tox. `ensure` alone does not replace a
  same-version daemon binary; install the rebuilt binary with `install --force`.
  Skill edits require rendering and
  staging `claude-skills/` in the same change.
- GitNexus is available but this repo is not indexed; source-derived checks
  are the fallback. The user requested implementation, not a new planning gate.

Latest automated gates: 399 bridge tests + 21 subtests, workspace tests/build,
Clippy warnings denied, rustfmt, documentation links and six script tests pass.
Final installed replay passed under
`/tmp/tdmcp-timing-ewhgIN/implementation/final-installed`; final reconnect and
worker regression evidence is `restart-final.json` / `thread-guard-final.json`.
Range preflight also passed live without reset/frame/flag changes. Initial
implementation is complete; native platform coverage remains unverified.
Changes are uncommitted; generated Claude skill
files are staged per AGENTS.md. Preserve all earlier/user edits.
