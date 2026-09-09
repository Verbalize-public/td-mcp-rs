# Timing accuracy: investigation and implementation plan

Status: initial implementation complete and verified (2026-09-09). Shared
deferred runner, inspect metadata, timed capture, record/read lifecycle,
schemas and diagnostics are implemented. Live acceptance is TD 2025.32460
on Wine/Linux; native platform coverage remains explicitly unverified.
The original investigation/design below is retained as rationale; the public
API is now documented in `docs/CONTRACT.md` and `timed-capture` skill card.

## Historical context (reported in the original proposal)
The touchdesigner timeline  (play/pause/step by and state) is reachable programatically  Where it lives: the timeline is a timeCOMP at /local/time, reachable as root.time. (Note: op('/time') — the path you'll see in older docs — returns None in TD 2025; it's under /local now.)

What I actually did in your session:

Action  Code    Observed result
Read transport  root.time.par.play.eval(), .frame, .seconds, .rate  playing, frame 463, 60 fps, range 1–600
Pause   root.time.par.play.val = 0  paused ✓
Step +5 frames  root.time.frame = 463 + 5   read back 468.0, .seconds → 7.7833 ✓
Single-frame step   root.time.frame = 463 + 1   464.0 ✓
Restore frame + play set back   frame 463, playing — state as found
The surface that worked: par.play (play/pause toggle — the same thing the bottom transport bar drives), frame (read/write scrub & step while paused), seconds, rate, tempo, par.start/par.end, plus cook-state members (cookFrame, cookedThisFrame, cookedPreviousFrame) and process-absolute time (absFrame/absSeconds, and the global absTime — which is process-lifetime time, not the timeline). Any COMP can own its own TimeCOMP for hierarchical timing, and the same controls apply there.

One operational caveat from the play-state card, confirmed by this design: a paused timeline stalls most cooks, so inspect/capture reflect the paused frame — always check/set play state before trusting a capture or grading motion.


## Problem

The main issue when it came to agentic work on touchdesigner is probably the lack of proper and precise timing awarness. Eg
- Lets say the agent want to verify a transition/fading effect. He want to take the initial state, mid state and final state. To do so he must rely on thre call to `inspect` or python execution. The issue is that by using inspect timing is 100% certain to be wrong and non reproducible. We could do the proper thing in python but the agent might not do it intuitively and if we want reproducibility better doing this programatically (at least as a primary/best effort approach)
- The agent tend to ignore playing state entierly where its the main thing that affect playing ect
- Beign able to do step by step execution is what fell the most like a debugger and could be such of a great help for an agent to work efficiently

## Agreed direction: reset, then advance

Precise temporal verification starts from an explicit reset and advances
sequentially through every intervening frame. Seeking or moving backward is
not the basis of this feature: the resulting state depends on the network,
and changing the timeline position does not reconstruct its history. The
frame assignments recorded above establish transport access only, not correct
stateful advancement or cooking.

Every stateful component must provide a reset signal, normally exposed through
a custom parameter, that returns all owned state to its documented initial
condition. Parent components propagate that signal to stateful children. By
default, project integration wires the global project reset to subsystem and
component resets for development and repeatable runs; those same public
controls remain usable locally or with application-specific reset sources.
Reusable internals must not hard-code the global reset's project path.

Reset initializes runtime state while preserving configured controls and
seeds. It is not a parameter-default restore or a reseed operation. Its contract
includes any initialization order, hold/release, and cooks needed before the
initial state can be observed. The reset entry point and behavior must be
documented in the network and verified from accumulated state, both locally
and through the integrated project reset.

The timing operation should consume this public reset contract. For a fade,
reset once, complete initialization, then advance and capture initial, middle,
and final samples at specified frame offsets, after cooking. Record observed
frames and play state; replay from reset to revisit an earlier sample. A frame
jump, repeated assignments in a Python loop, or wall-clock sleeps alone do not
prove that intervening cooks and callbacks ran. Restoring a previous frame and
play flag also cannot restore the prior state of a simulation.

Reproducibility depends on controlled inputs, seeds, clocks, and advancement.
These were investigation gates in the original proposal; implementation and
live results are recorded in the final section below.

The shared skill contract is authored in
`skills/templates/touchdesigner/reference/reset-state.jinja.md` and routed from
general operation, network design, component/custom-parameter authoring,
completion checks, play state, and visual verification. Missing or incomplete
reset behavior fails completion for stateful work; unavailable live evidence
must be reported as unverified.

## Tool design (original proposal, now implemented)

- `inspect`: additive timing metadata (play state, timeline frame/rate,
  effective time source, available cook metadata). Ordinary inspection does
  not change transport. Keep existing response fields compatible.
- `capture`: optional play/pause/step actions and a prepared sample schedule.
  The agent chooses the sequence; TD executes it locally. Support explicit
  frame offsets and a regular count/interval schedule compiled to offsets.
  Optional inspection at each sample pairs parameters/errors with the image
  without another MCP round trip. Plain capture retains its current behavior.
- `record`: a dedicated tool for exactly N consecutive frames from one explicit
  TOP output. Use the same reset/advance runner as capture. Start returns a job
  identifier; status and cancel remain available while the job owns its PID.
  Return an artifact reference, frame count, dimensions, codec and FPS only
  after finalization. Remote callers need artifact retrieval, not just a remote
  filesystem path. Initial scope is video without audio.

Capture request (implemented schema):

```json
{
  "pid": 123,
  "path": "/project1/fade/out1",
  "timing": {
    "reset": {"path": "/project1", "parameter": "Reset"},
    "sampleFrames": [0, 30, 60],
    "after": "pause"
  },
  "inspect": {
    "paths": ["/project1/fade"],
    "include": ["params", "errors", "warnings"]
  }
}
```

Offset zero is the initialized state after reset completion. Initialization
and warmup frames must be accounted for separately. Capturing offsets 0, 30,
60 advances every intervening frame; recording 120 frames includes offsets
0 through 119. At 60 FPS the latter produces a two-second video, regardless
of wall time spent rendering. FPS defaults to the selected timeline's rate;
simulation rate and output playback rate must not be silently conflated.

Each sample reports requested offset, observed timeline frame, play state,
time source, available cook metadata, image and optional inspection. Failed
or partial schedules report actual progress, not invented sample positions.

## Source findings already checked

- `bridge/tdmcp_bridge/task_queue.py::process_pending` dispatches a request
  synchronously in one main-thread invocation. A deferred runner must yield
  to TD and retain ownership until its actual terminal state.
- The pause-safe `_pump` is scheduled with a 50 ms wall-time delay and a
  separate time reference. It provides request servicing, not exact frame
  advancement or a post-cook sampling boundary.
- `bridge/tdmcp_bridge/capture.py::handle_capture` reads/encodes the current
  output; it has no time schedule. `inspect.py::handle_inspect` has no timing
  snapshot and performs its own structural reads.
- `crates/tdmcp-mcp/src/rmcp_handler.rs` promotes one top-level `imageBase64`
  for capture. Multiple samples need ordered image promotion and bounded
  results or artifact references.
- PID exclusivity and session gates currently follow individual calls.
  Asynchronous jobs need lifetime ownership plus accessible status/cancel.
  Existing timeout behavior is not cancellation; do not release ownership
  while a deferred callback can still mutate the network.
- Official [Movie File Out TOP documentation](https://derivative.ca/UserGuide/Movie_File_Out_TOP)
  describes stop-frame recording and Add Frame. Encoding availability depends
  on the installation/license/hardware. The live qtrle result below validates
  one backend configuration, not every listed codec or platform.

## Investigation workflow and acceptance matrix

Use real MCP, a discovered PID, and an owned scratch project/network. Read
the live API and parameters before selecting mechanisms. Save evidence and
curated runnable Python under `scripts/timing_probe/`; keep observation data
separate from examples. Restore transport settings and clean up only resources
created by the probe. Do not refactor production tools in this investigation.

| Probe | Required evidence | Status |
| --- | --- | --- |
| Transport | Read/play/pause; root vs component time; paused MCP responsiveness | Root pause/resume and paused MCP verified; independent component transport pending |
| Advancement | Sequential callback/cook counts; contrast with frame assignment loop | Forward increments with a full callback yield verified; same-call loop fails |
| Sampling | Initial/middle/final capture plus inspect at the same controlled sample | Verified offsets 0, 3, 6, unchanged frame during inspection |
| Reset | Public pulse reaches child state; reset from dirty state; repeated reset/replay agrees | Verified parent/child pulses and identical hashes on replay |
| Stateful cooking | Counter, transition, and feedback/history output progress without skipped/duplicate updates | Verified counter/fade/feedback at seven samples |
| Time sources | Timeline vs process time; local time; frame-range boundary behavior | Pending |
| Recording | Supported codec; exactly N frames; first/last content; readable finalized video; FPS/dimensions | Seven decoded qtrle frames, 64×64, 60 FPS; first/last verified. Other backends pending |
| Failure cleanup | Cancellation, invalid/deleted source, encoder failure, cleanup and final transport state | Mid-run cancel finalized a two-frame partial movie; invalid count/mechanism/existing file rejected before mutation. Source/encoder failure and boundary races pending |
| Job integration | Source-level design for ownership, disconnect/timeout, status/cancel, result bounds | Pending; needs future refactor for end-to-end tests |
| Remote artifact | Transfer/retrieval design and explicit untested-platform notes | Pending; no artifact API yet |

Do not use sleeps or repeated Python assignments as evidence that TD advanced
and cooked. A callback boundary is also only a hypothesis until tested against
observable state and captured output. Stop a probe after three failures with
no new evidence; document the limitation and continue independent checks.

## Refactoring sequence after investigation

1. Choose the stepping and post-cook sampling mechanisms from live results;
   preserve curated examples as reproducible acceptance fixtures.
2. Introduce a shared deferred frame runner with explicit reset initialization,
   sample offsets, deadlines, progress, cancellation, and cleanup.
3. Add timing metadata to inspect and optional transport/schedules to capture;
   promote multiple images with sample identity and enforce payload limits.
4. Add the record job and video artifact lifecycle; reserve the PID until
   callbacks and encoder cleanup finish. Handle frame jumps, external transport
   changes, source loss, range ends, and late callbacks explicitly.
5. Add schema/catalog/bridge tests, live acceptance rows, contract and skill
   updates. Repack the bootstrap tox if baked callbacks change; regenerate
   Claude skills for any skill edit. Build/install/restart per AGENTS.md.

## Live findings and curated examples

Scratch project: `/tmp/tdmcp-timing-ewhgIN/timing.toe`, first spawned PID
292335, TD 2025.32460 on Wine/Linux. Transport/API discovery succeeded through
MCP. The first combined pause/non-realtime setup returned a reset-fixture
error, then inspection timed out and the bridge disconnected. This does not
establish that pause or non-realtime is the root cause.

Thread-safety investigation found a concrete bridge defect: global logtap and
execute-python stream wrappers forwarded worker output directly into TD's
native Textport stream before the existing debug-DAT thread guard. Worker
teardown/error logging could therefore enter TD's UI context. Regression
tests reproduced those off-thread native calls. The fix defers native stream
writes/flushes to the main-thread pump and guards TD dispatch entry points.
Live validation passed on replacement scratch PID 328851: worker stdout and
stderr completed with capture both enabled and disabled; all native writes
were deferred; the queue drained to zero; off-main dispatch was rejected.
Both connected TD processes automatically reconnected after daemon restart.

A second defect was established: the pause-safe pump could return inside its
50 ms rate-limit window without scheduling a successor, leaving its scheduled
flag true. Live observation showed eight queued stream operations and no pump
Run despite the flag. A regression reproduced this early-tick failure; the
fix always schedules the next tick while connected. With both fixes installed,
paused inspection succeeded with realtime on and off, before the recovery
timer fired. Play/realtime settings were restored. Evidence is under
`/tmp/tdmcp-timing-ewhgIN/thread-guard-fixed.json` and
`thread-guard-nonrealtime.json`; rerun via `scripts/live_thread_guard_smoke.py`.

The corrected fixture uses `reset_callback.par.op.expr = 'parent()'`;
relative `.` had incorrectly selected the callback DAT. Reset initialization
takes one explicit frame with state advancement disabled. A full independent
callback iteration after each single forward frame assignment is necessary:
sampling at the assignment's end-frame callback observed state one step late.
The single-call five-assignment loop advanced frame numbers without any state
callbacks. Neither case is a valid substitute for sequential advancement.

On replacement PID 328851, reset/replay produced identical hashes for a fade
and feedback accumulator at offsets 0–6, with six frame-start/end pairs. Real
capture and inspect handlers ran together at offsets 0, 3, 6 without moving
the sampled frame. Root absolute time also stopped while root time was paused;
do not treat it as an unconditional wall clock. The TDResources time reference
continued servicing callbacks independently.

Stop-frame recording exposed an additional boundary: seven Add Frame requests
followed by immediate record-off encoded only six images. Yielding one callback
iteration before closing produced seven decodable qtrle frames, 64×64 at 60
FPS, with decoded red means from 0 to 0.5 matching the fade. `writeCount` did
not report encoded frame count. Finalized-file verification is mandatory; a
nonempty file or successful Add Frame pulse is insufficient evidence.

Curated, explicitly scoped examples and reproduction commands are in
[`scripts/timing_probe/README.md`](../scripts/timing_probe/README.md). They are
investigation fixtures, not shipped timing tools. Local-time/range behavior,
failure races, production job ownership and remote artifacts remain acceptance
gates; the urgent bridge fix does not imply those workflows are complete.

## Implementation results (2026-09-09)

The user approved implementation after the investigation. Production code now
uses `bridge/tdmcp_bridge/timing.py` for both deferred capture and record jobs;
`video.py` verifies bounded finalized MOV metadata. All TD calls, scheduling,
reset/cooking and native recorder lifecycle stay on the main thread.

Implementation decisions:

- Reservation lives at TD bridge dispatch, across callers and callbacks. Rust
  still gates each RPC; keeping an RPC open would prevent status/cancel from
  being serviced. Fleet queue entries are not job progress.
- Timed capture is TOP-only, with optional paired inspection and ordered MCP
  image promotion. Plain capture is unchanged. Status/cancel without jobId can
  recover active/latest results after a lost start reply.
- Initial recording is qtrle MOV at the effective timeline FPS, no audio, with
  N samples rather than N advances. Creating the recorder as a sibling is
  necessary for wiring and inherited local time; creation under the bridge's
  unrelated COMP silently dropped the input wire.
- Native container finalization is asynchronous after record-off. Keep the
  reservation while waiting (bounded by 30 seconds), then verify metadata.
  Preserve partial artifacts only when verified. Cleanup failure retains
  ownership for cancel retry rather than allowing conflicting mutations.
- Opaque job-based chunk reads support federation. Eight retained jobs,
  capture/video/read budgets and explicit release bound memory/disk/results.
  Reload cancels callbacks and invalidates/removes retained private artifacts.
- Working range ends and system clocks are guarded. Source/clock changes and
  deadline violations pause/fail instead of silently seeking or skipping frames.

Live evidence on owned PID 328851, TD 2025.32460/Wine/Linux:

| Acceptance | Result |
| --- | --- |
| Reset + fade/feedback replay | Identical PNG hashes on repeated 0/3/6 schedules, distinct samples |
| Paired inspection | Same sample frame, fade parameter values 0/0.5/1 |
| Video and retrieval | Seven tool-downloaded frames decoded by ffmpeg, 64×64/60 FPS; red mean 0→0.5 |
| Cancellation | Three-frame partial movie finalized; no callbacks/recorder left |
| Source loss / seek / play | Failed with interrupted diagnostics and paused cleanup |
| Broken encoder input | Failed with native recorder error and cleanup |
| Deadline | Failed with interrupted diagnostic, ownership released after cleanup |
| Local time | Three-frame video and 0/1/3 capture offsets; root frame unchanged |
| Range preflight | Crossing the end rejected with unchanged frame, reset log, component count and transport flags |
| Cross-session ownership | Conflicting inspect rejected as timing.busy; status recovery and cancel work across clients |
| Reload/reconnect | Paused, non-realtime scratch reconnects without UI recovery; old recording job, callbacks and recorder retired |
| Federation | Fake-peer integration verifies remote record start and artifact chunk routing |

Evidence root: `/tmp/tdmcp-timing-ewhgIN/implementation/` (`live-d`, `failures.json`,
`local.json`, `ownership.json`, `restart.json`). Reproducible scripts and limits
are in `scripts/timing_probe/README.md`.
Native Windows/macOS, other codecs, audio and a universal artifact API are
explicitly outside this initial backend's verified scope, not silent successes.
Restart acceptance found and fixed two additional lifecycle defects:

- Bootstrap purges bridge modules before importing a fresh generation. An
  interpreter-local timing runtime anchor now retires old jobs before admitting
  new ones; a regression reproduces the real fresh-import path, not only reload.
- The reconnect watchdog had an independent clock but lacked `wallTime=True`.
  Its nominal two-second retry could stretch under paused/non-realtime playback.
  The corrected callbacks were packed into bootstrap.tox inside TD, stamped,
  rebuilt and force-installed. Existing projects need a fresh tox drag-in to
  receive this baked callback fix; only the owned scratch was updated live.

Verification: workspace build/tests and Clippy with warnings denied pass;
399 bridge tests and 21 subtests pass, including both lifecycle regressions.
Schema goldens and generated Claude skill parity pass in the workspace suite.
The installed-build replay in `final-installed/summary.json` independently
decoded all seven frames after chunk retrieval, with red means
0, 0.08333, 0.16648, 0.25098, 0.33327, 0.41667, 0.5. Repeated fade/feedback
PNG hashes matched, and paired inspection observed each scheduled frame.

Final reinstall and lifecycle checks passed again after removing the runtime
anchor's temporary reference to the old module (avoiding retained generations).
Evidence: `restart-final.json` and `thread-guard-final.json`. Build and installed
daemon SHA-256 matched. Live bridge imports used the configured development
repository path; daemon transport was the rebuilt installed binary.

Cleanup removed only the owned `/project1/timing_probe` network and private job
artifacts, confirmed no timing runs, and restored its original playing/realtime
flags. Downloaded evidence remains; the fixture is reproducible from the scripts.
No unrelated project was modified, and no timeline history was rewound.
