# Mutation zones (TD)

Build in a purposefully-named component from the first mutation. Package per
[component-checklist](tdmcp://docs/component-checklist) +
[custom-parameters](tdmcp://docs/custom-parameters). Do not stage in a throwaway
container and promote later.

## How to get a zone

| Kind | When | How it becomes verified |
|------|------|-------------------------|
| **Self-created (default)** | New experiment / COMP | Create a descriptively-named Base/Container (often under `/project1`). Confirm with `inspect`. |
| **User-authorized subtree** | User names an existing path | Resolve live with `inspect`; refuse if missing / ambiguous. |
| **Editor-hinted** | User is browsing a network; no path named yet | Call `editor_context` → take focused pane `ownerPath` (and optional `selection`) as a **hint only**. Still confirm with `inspect` before mutating; do not treat pane focus as authorization. |

## Preview

1. Terminal visual on a named "Out [FAMILY NAME]" component (each family get a dedicated "In"/"Out" component) / null inside the zone.
2. Zone COMP: `par.opviewer = './out1'` (`./` = child inside this COMP); `viewer = True`.
3. Look claims: `capture` tool on the terminal

## Safety

- Never destroy nodes you did not create without explicit approval by the user.
- Never mutate outside the current zone without fresh explicit authorization.


---

**Canonical:** `tdmcp://docs/mutation-zones` 
