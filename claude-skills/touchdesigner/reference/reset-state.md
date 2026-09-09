# Stateful component reset

Required when building or changing stateful behavior: feedback, accumulators,
simulations, timers, transitions, history buffers, or script-owned state.
Apply this contract to the components in the task's scope.

## Public reset contract

- Every stateful component **must expose a working reset signal**, normally
  a custom `Reset` pulse on its boundary COMP. Reuse an existing equivalent
  public control when present. A button with no connected behavior is a defect.
- Reset must return **all owned state** to a documented initial condition,
  including stateful children and script storage. Keep the user's configured
  controls; resetting runtime state is not restoring parameter defaults.
- Define any initialization order, reset hold/release, and cooks needed before
  the initial output is ready. Repeating reset must return to the same initial
  condition. Keep configured seeds stable; reseeding is a separate operation.
- Wire reset to the actual state-clearing mechanisms. Use a pulse callback
  when event propagation is needed; a parameter's existence or a value
  expression alone does not prove pulse delivery. Verify the live operator API
  with `api_help` before choosing its reset mechanism.

## Project integration

- **Default wiring is project reset → subsystem reset → component reset.**
  Reuse the project's existing global reset control and conventions. When
  building a project/subsystem with no reset entry point, provide one in the
  owned scope and document how its parent connects to it. Project reset is an
  explicit network control; do not assume a built-in reset path or parameter.
- Parent components propagate reset through their children's public controls.
  Reusable internals must not hard-code a project-global path. Keep the local
  reset usable standalone and connect the global signal at integration time.
- Allow that same local reset to be driven independently or by another reset
  source when the application needs it. Preserve intentional existing routing
  and document it. Do not add unrelated components to a reset domain.
- Document the reset entry point, affected state, initial condition, completion
  behavior, and upstream reset source in the COMP's comment/help or nearby DAT.

## Verification: reset, then advance

1. Exercise the component until it has accumulated state. Trigger its public
   reset, allow the documented initialization to complete, and inspect/capture
   the initial condition. Repeat reset to check that it remains valid.
2. Exercise the project/subsystem reset and verify that the same reset reaches
   the component and its stateful children. For a standalone component, verify
   its local contract and identify project integration as not yet exercised.
3. For temporal checks, start from reset and advance sequentially through every
   intervening frame, allowing the required cooks and callbacks to run. Sample
   after cooking at stated frame offsets from reset. Repeat with the same
   controls, seeds, inputs, and advancement schedule; compare observations using
   a tolerance appropriate to the effect. Report uncontrolled inputs or clocks.

Seeking, moving backward, setting the timeline to its start, and restoring a
previous frame/play flag do **not** reconstruct accumulated state. To revisit
an earlier sample, reset and replay forward. Separate MCP calls or wall-clock
sleeps alone do not establish exact frame spacing; report timing as unverified
unless the advancement and observed sample frames were checked.

Missing, disconnected, or incomplete reset behavior fails completion for
stateful work. If live verification is unavailable, report it as unverified
instead of claiming reset or replay works.

## Related

- [`component-checklist`](./component-checklist.md) — component boundary and reuse
- [`custom-parameters`](./custom-parameters.md) — public reset control
- [`play-state`](./play-state.md) — transport and cooking
- [`definition-of-done`](./definition-of-done.md) — completion evidence
- [`look-grade`](./look-grade.md) — temporal capture evidence

**Canonical:** [`reset-state`](./reset-state.md)