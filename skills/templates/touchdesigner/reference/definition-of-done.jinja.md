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

- Inspect the touched parent COMP after the final mutation. Account for
  errors and warnings, separating existing issues from changes you made.
- Confirm the intended parameters, wires, and component boundaries.
- Capture the output for appearance claims and view the resulting image.
- For new reusable components, check relative references, In/Out pins, and
  the exposed control parameters.
- Keep the changed nodes readable and free of accidental overlap. Re-layout
  only the relevant subtree, at a useful point in the task.
- Stop repeating a failed probe after three attempts without new evidence.

Scale verification to the task. A parameter adjustment does not require
packaging a component or reorganizing an unrelated network.

## Related

- {{ skill("network-design") }} — layout and relative references
- {{ skill("component-checklist") }} — reusable components
- {{ skill("look-grade") }} — visual verification
- {{ skill("node-comments") }} — durable design intent

**Canonical:** {{ skill("definition-of-done") }}
