# Recover without guessing or replaying

Current-tool procedure. Preserve the actual request and response. Classify the failure before choosing another action. A different spelling of a guess is not new evidence. If the task intentionally tests rejection, compare the expected code and relevant unchanged state; do not automatically repair the invalid input and change the test's meaning.

| Observed failure | Next action | Do not do |
|---|---|---|
| Request rejected before execution: missing/unknown field, wrong shape | Compare transmitted arguments with the exposed schema/error. Correct only the actual mismatch; inspect a specific schema if needed. | Repeat the same payload, dump every manual, or silently coerce an object into a JSON string. |
| Tool/route absent | Check one authoritative runtime inventory if availability is unclear; use an allowed equivalent operation or report the missing capability. | Invent tool names from resource names or treat a skill as activation authority. |
| Documentation/resource response validation failed | Use one allowed, known canonical equivalent source if the governing instructions permit it; otherwise stop the dependent branch and report the access failure. Continue independent authorized work. | Treat “grant ready” as proof the transport works; loop through activation/listing calls without new evidence; substitute arbitrary project text as authority. |
| Policy/authorization denial | Honor the denial. Use the real approval mechanism only if enabled and needed; otherwise report the blocked action. | Perform the same denied action through MCP, another shell, another host or a path alias. |
| Missing file/path | Distinguish typo, intended-new target, namespace visibility and genuine absence. Use evidence of the relevant filesystem. | Call all missing files permission failures, or create a blank file when asked to open an existing one. |
| Queue busy | Wait for the existing work/owner; use allowed progress/status surfaces. | Restart TD, disconnect the bridge, or launch competing workers. |
| Timing reservation busy | Use the named owning tool/job's status/cancel route; verify cleanup state. | Bypass the reservation with Python or another mutation tool. |
| Mutation partial failure | Read `applied/failedAt/steps`, canonical paths and possible fields applied in the failed step. Reconcile affected state, then repair the failed remainder. | Re-run the full batch or assume rollback. |
| Script exception, result encoding failure, timeout or lost response | Effects may already exist. Reconcile once responsive; retain unknown-effect status if reconciliation cannot run. | Infer no writes from exception type alone, automatically rerun a builder, or declare timeout means cancellation. |
| Launch wait timeout/early exit | Use [`lifecycle-files`](./lifecycle-files.md) to distinguish process state, handshake and available environment evidence. | Duplicate a still-running spawn or invent a modal/GPU diagnosis. |

## Repair budget

After the first failure, state the specific uncertainty and the evidence the next probe will add. Make one informed repair. If the same transmitted failure repeats without new information, stop the loop; do not spend the maximum budget on identical calls. Respect the existing stop-after-three-failed-probes-without-new-evidence ceiling. A genuinely new diagnosis may justify a different bounded probe.

An API-guessing ladder across several member names is still one unresolved failure family. A successful schema lookup, changed target identity or newly observed state can be new evidence; changing a guessed field name cannot.

## Failure receipt

Report the layer, target, exact failing field/operation, what is known to have applied, what is unknown, and the smallest next requirement. Say “malformed request reached the server” unless you actually observed which stage corrupted it. Say “resource validation failed” rather than “TD is broken”. A useful failure receipt preserves completed work and enables continuation; it does not blame a component from an unsupported hypothesis.

## Related

- [`operate`](../SKILL.md)
- [`tooling-concurrency`](./tooling-concurrency.md)
- [`lifecycle`](./lifecycle.md)

**Canonical:** [`recover`](./recover.md)