# Definition of Done (structural)

Verify the requested behavior at the surface it affects. Use
{{ skill("look-grade") }} for visual and performance claims.

## Evidence

| Result | Meaning |
| --- | --- |
| Verified | Current inspection or capture supports the claim |
| Failed | Current evidence contradicts the requested behavior |
| Unverified | The relevant surface was unavailable or the check was not run; explain why |

Do not turn missing evidence into a success or claim a defect solely because
a check could not run. A node comment records intent; it is not proof.

## Check the changed network

- Build a bounded, deduplicated verification set from returned canonical changed
  paths (including auto-renames), affected destinations/consumers and relevant
  parents. Inspect it in small batches, at most 256 paths per call; do not walk
  unrelated subtrees. A parent inspection does not recursively check children.
- Include both `errors` and `warnings`; require each node's `ok` and
  `observations.errors.available` / `observations.warnings.available` before
  treating empty arrays as clean. Missing/unavailable observations are unverified,
  not clean. Account for messages, separating existing issues from your changes;
  tool-level `ok` alone does not establish node health.
- On partial failure, read `applied`, `failedAt` and `steps`. Successful steps
  remain applied, and the failed step may itself have partially applied fields.
  Inspect its possible effects before repairing from `failedAt`; do not replay
  successful steps or assume skipped steps ran.
- Exclude deleted paths from success targets. Verify absence through the surviving
  parent's child roster, paging until coverage is sufficient to establish absence;
  if an ancestor was deleted, use its surviving parent. Check affected destinations
  for disconnected wires. A missing node response alone is not deletion proof.
- Confirm the intended parameters, wires, and component boundaries on the actual
  changed nodes/destinations. Use `paramNames` for focused value reads and check
  `paramsSelection` plus `evaluation.available`; check wire observation availability.
  For rosters, follow `childrenPage.nextOffset` and account for truncation:
  `complete` means this response covers the whole roster from offset 0, not that
  the last page was reached. An empty out-of-range page is not an empty COMP.
  Offsets have no snapshot isolation; recheck coverage if the graph changes.
- Verify shader compilation separately on affected consumers with `content` /
  `shaderDiagnostics`. Require affirmative `compiled` evidence; empty operator
  errors, `unknown` or `unsupported` do not prove compilation. Content reads can
  force recompilation. Compilation success does not prove appearance or timing.
- For stateful work, require a working public reset and default project-reset
  integration (or documented intentional routing). Verify accumulated state
  clears, child resets propagate, and reset then advance reproduces the checked
  behavior under controlled inputs. Follow {{ skill("reset-state") }}:
  a missing/incomplete reset is a failure; an unrun check is unverified.
- Capture the output for appearance claims and view the resulting image. For
  temporal claims, collect timed evidence via {{ skill("timed-capture") }};
  a still image is not proof of sequential behavior.
- For new reusable components, check relative references, In/Out pins, and
  the exposed control parameters.
- Keep the changed nodes readable and free of accidental overlap. Re-layout
  only the relevant subtree, at a useful point in the task.
- Stop repeating a failed probe after three attempts without new evidence.

Scale verification to the task. A parameter adjustment does not require
packaging a component or reorganizing an unrelated network.

## Handoff and persistence

Report structure, appearance, temporal behavior and persistence separately.
A working live network is not a saved `.toe`; a completed recording is not yet
a durable downloaded file. For requested deliverables, name the saved path and
verification performed, or say explicitly what remains unsaved/unverified.
Save only to the authorized destination; preserve unrelated project contents.
Report the final frame/play state and any intentional transport changes.
For partial or stopped work, identify completed outputs and the remaining scope.

Keep failure receipts precise: daemon/PID, TD build where relevant, operator
path, exact arguments, response and validation layer. Separate an observed
failure from its suspected cause. A failed diagnostic does not by itself mean
the user's requested result failed, and a failed recipe on one topology/build
does not establish that an operator family is universally broken.

## Related

- {{ skill("network-design") }} — layout and relative references
- {{ skill("component-checklist") }} — reusable components
- {{ skill("look-grade") }} — visual verification
- {{ skill("node-comments") }} — durable design intent

**Canonical:** {{ skill("definition-of-done") }}
