# Capture and wiring acceptance fixture

`capture_wiring.json` contains **unbound arguments**, not ready-to-send MCP
requests. It has no PID, no sticky target, and no global project path. Rust tests
validate its steps and argument shapes against the actual tool types after
binding a test target. Native execution was exercised on **TD 2025.32460 under
Wine/Linux**. See [live validation](../../../docs/KNOWN_ISSUES_LIVE_VALIDATION.md)
for scope, failures, corrections and persistence evidence.

## Preconditions and ownership

1. With enabled TD MCP access, discover the daemon/install and create a new owned
   scratch project using `spawn_td` with the current advertised schema. Record
   the returned PID, daemon identity, project path, TD build and bridge package
   identity. Do not target an existing user project.
2. Create a new `baseCOMP` inside the scratch project. Read its returned canonical
   path; if TD renames it, use the actual path. Inspect that it is empty. Keep
   that path and ownership evidence for cleanup; do not infer it from a name.
3. Bind that explicit numeric `pid`, optional `daemonId`, and `contextPath` to
   each tool call below. Never send the JSON file wholesale or omit its binding.
   Preserve the transmitted request and result, not just the intended arguments.

## Sequential acceptance

| Stage | Call | Required observation |
| --- | --- | --- |
| Build | `mutate_nodes` with `steps` from `build` | All seven steps applied; canonical paths remain under the owned root; input indices are explicit. Stop and inspect after any failure; do not replay the batch. |
| Inspect | `inspect` with the `inspect` fields | Available input observations show source_a at merge input 0 and source_b at input 1, with merge feeding out. Available errors/warnings are empty. Parameter metadata is readable or explicitly unavailable. |
| Capture | `capture` with the `capture` fields | Correct PID/path, PNG content actually visible to the verifier, bounded dimensions. Default constant images may look identical; this is not a shader or alpha correctness test. |
| Guard failure | `mutate_nodes` with `steps` from `protectedRewire` | `tdmcp.wire.input_occupied`, applied=0, failedAt=0. Re-inspection proves input 0 still references source_a; source_b did not replace it. |
| Explicit rewire | `mutate_nodes` with `steps` from `legacyRewire` | Success plus occupied-input warning and previousConnections identifying source_a. Re-inspect actual TD behavior; do not infer displacement solely from the warning. |
| Timed capture | `capture` with the `timedCapture` fields | On this owned timeline, confirm room for offsets 0 and 1 first. Poll the same PID/jobId until terminal; state must be complete with the expected offsets, clock identity and viewable images. This stateless test does not establish feedback/trail semantics. |
| Persistence | Save the owned project; reopen a new owned instance via MCP | Record the new PID/project identity and repeat inspect/immediate capture without rebuilding nodes. Verify wiring survived. |

Timed capture changes playback and leaves it paused by design. Do not issue other
bridged work while the job owns the PID. On timeout, reconcile status rather than
starting a duplicate; cancel only owned jobs and verify cleanup before mutation.
Retain evidence before releasing artifacts. Stop after three failed probes with
no new information.

## Native fixture scripts

Send each file as `execute_python.script`, with the confirmed PID and an explicit
`contextPath` pointing to the owned audit COMP. These are staged operations, not
a blind replay loop; inspect partial state after any error. The stdio transport
helper is `scripts/mcp_stdio_probe.py`; `scripts/live_evidence.py` records exact
requests/results and extracts image blocks without retrying calls.

| Source | Prerequisite / purpose |
| --- | --- |
| `build_pop_scene.py` | Creates a fresh `scene` with a colored POP ring, camera, unlit MAT and render output. Refuses an existing scene. |
| `build_feedback_scene.py` | Creates a fresh `temporal` accumulator with public Reset and numeric Red observation. |
| `add_trail_scene.py` | Requires scene; inserts Trail POP and a public reset/count observation. |
| `build_trigger_scene.py` | Creates a fresh resettable threshold-crossing signal fixture. |
| `finalize_fixture.py` | Requires the accepted scene/temporal/trigger_test under the task-owned root marker; creates a clean sibling verified_fixture with Reset/Intensity/Scale. |
| `rebind_fixture.py` | Rebinds known fixture references owner-relatively and verifies their evaluated paths remain inside the root. Useful as a relocation acceptance check. |
| `path_probe.py` | Requires named geo/shaderpop/render/cam discovery nodes; changes and restores only its inspected parameter contexts. |
| `chop_plot_probe.py` | Requires discovery nodes chopgen/noise; tests P vs P(1) mapping and restores settings. |
| `signal_probe.py` | Requires lfo/noise/chopgen and scene; bounded measurements, restoring edited values. |
| `bloom_probe.py` | Requires scene with a bloom TOP connected to render; factorial coverage/brightness/alpha probe with restoration. |

The public/global reset fixture used three initialization frames before sampling.
Its exported `.tox` was loaded in a separate blank project with no audit tree and
reproduced 256/320/576 Trail points at offsets 0/1/5. Intensity and Scale changed
measured pixels/bounds; no inert control is presented as functional.

Do not change a parameter mode using an unverified string **after** assigning
its expression. That left stored expression text inactive in CONSTANT mode in
the live probe. Assign mode before expression and read back mode/evaluation.
Deleting an OP can also delete docked DATs; cleanup checks OP validity.

## Evidence and cleanup

Retain source/build hashes, exact requests/results, canonical root, PID/daemon,
project identity, clock/play state, image provenance, and pass/fail/unverified
judgments. The fixture is only the T1/T2/fixed-context transport baseline.
Separate POP/shader/menu/alpha/feedback/trail/bloom experiments and their actual
results are in the live validation report; do not promote the basic wiring
fixture alone into proof of those behaviors.

Only after saving evidence and settling jobs, remove the exact COMP created by
this run if cleanup is desired, or retain the owned saved project for regression.
Never delete a presumed default path, unrelated operators or a pre-existing root.
