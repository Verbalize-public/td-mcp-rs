# Visual, temporal and performance evidence

Current-tool procedure. The requested claim chooses the evidence. Do not do every check for every task.

## Close the visual loop early

For appearance-driven work, identify the reference's decision-changing features, produce a viewable first version early, and compare it before investing in optional architecture, packaging or optimization. This is an iteration point, not permission to deliver only a prototype. Use human feedback where a material artistic choice needs it; otherwise continue authorized work against the supplied reference and state subjective limits.

A still image supports only the observed appearance at that sample. View the returned image through an available permitted path; if the model cannot see it, use an authorized visual verifier or leave the claim unverified. A stored attachment, screenshot filename, shader success or a worker's claim is not visual inspection.

Original capture data and the model's preview may differ in encoding, size, color/depth or quality. Describe what was actually inspected. Use exact artifacts/numerical evidence for precision claims; do not treat a normalized preview as an exact pixel oracle. Black/uniform output can be intentional.

## Motion and stateful behavior

Read the current timing/reset contract when exact sampling or accumulated state matters. A public reset is required for newly authored/changed stateful behavior; reuse the existing equivalent control. Do not add or exercise a project-wide reset merely to make an unrelated scalar edit or read-only screenshot comply with a checklist.

For repeatable stateful tests: preserve controlled inputs/seeds, reset the authorized state, perform the documented initialization, and advance every intervening frame/cook before sampling. Seeking backward and restoring a play flag do not reconstruct simulation history. Do not sleep on TD's main thread while expecting it to advance.

Use current timed capture/record when supported. Poll progress without repeatedly fetching old samples:

```json
{"pid":123,"action":"status","jobId":"returned-job-id","includeSamples":false}
```

This is a `capture` status example. Use the actual owning tool/PID/job id; the real schema governs available fields. Status `ok:true` is not job success: check terminal state, error, actual offsets/counts and artifacts. A failure or cancellation can leave partial data. Retrieve and verify required output before release; the private job artifact is temporary.

Respect reservations and cleanup. Report the actual final play/frame state. “Restore” does not mean the entire user's scene/history was restored.

## Performance

Frame-stepped samples and movie FPS metadata do not establish live throughput. Measure the relevant output's behavior over wall time under stated resolution, inputs, viewer/consumer, warmup, runner and GPU conditions. Distinguish target cook rate, timeline advance, node cooking, CPU/GPU timing and presentation rate. Do not select the best headless number when the requirement concerns a visible viewer.

Use a controlled canary to separate an environment limit from the application's cost. Do not publish a universal operator/driver defect from one topology or an unverified sandbox environment. Finish with the {{ skill("verify-handoff") }}.

## Related

- {{ skill("operate") }}
- {{ skill("look-grade") }}
- {{ skill("timed-capture") }}
- {{ skill("reset-state") }}
- {{ skill("primer/performance") }}

**Canonical:** {{ skill("visual-timing") }}
