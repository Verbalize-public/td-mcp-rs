# Operator comments

Every TD operator carries a free-text `comment`. It is the only place a
network states *why* it exists — write it with `mutate_nodes`, read it with
`inspect`. Router: {{ skill("operate") }}.

## The surface

| | |
|---|---|
| Storage | `OP.comment` — a plain read/write `str` on **every** operator, every family. Not a `.par`, not a flag. Empty by default |
| Write | `mutate_nodes` `create` / `set` → `comment` field. An empty string clears it |
| Read | `inspect` returns `comment` on every node when non-empty (capped 1024 chars; `commentTruncated: true` when cut), and on each **child-roster** entry (capped 160 chars) |
| Python | `n.comment` (see {{ skill("python-api") }}) — use the `mutate_nodes` field instead unless you are already in a script |
| Travel | Copies with the operator (copy/paste, clone) and saves into `.toe` / `.tox` — the comment ships with the component |

Comments are metadata, never evidence: they are what a *previous* author
claimed, not what the network does. A claim about live behavior still needs
`inspect` / `capture` ({{ skill("definition-of-done") }}).

## Why comment at all — HARD RULE

A network is read many more times than it is built: by the next session with
no memory of this one, by another agent, by the user months later. `opType`
and node name already say **what** an operator is; wires and parameters are
recoverable by inspecting. **Intent is not recoverable.** Why this feedback
loop exists, which magic constant is load-bearing, what contract a terminal
null holds — all of that is lost the moment the session ends unless it is
written on the node.

**So: every operator you create that is not self-evident gets a `comment`, in
the same `mutate_nodes` batch that creates it.** Not a follow-up pass — the
follow-up pass is what never happens.

## What to comment

| Always comment | Skip |
|----------------|------|
| Every COMP hub (what subsystem it is, what its In/Out contract is) | Pass-through nulls in an obvious linear chain |
| Terminal nulls other things Select or reference — name the contract | A transform doing exactly what its name says |
| Feedback loops, integrators, anything stateful (what resets it) | Nodes whose whole role is one visible parameter |
| Any non-obvious parameter value, expression, or magic constant | Nodes already covered by the parent COMP's comment |
| GLSL ops and their stage DATs — what the shader is for | |
| DAT callbacks / extensions — what fires them | |
| Anything whose deletion would look safe but is not | |

The gate is the same instinct as {{ skill("opsketch-importance-gating") }}:
would a reader who has never seen this network guess wrong without it?

## How to write one

- **One or two lines.** The **first line must stand alone** — the child roster
  truncates at 160 chars, so the first line is what a reader sees when
  inspecting the parent.
- **Say the role and the why**, not the type. `noiseTOP` is already on screen.
- **Name contracts explicitly** when there is one: what reads this, what breaks
  if it is renamed, what must stay in sync.
- **No ceremony** — no dates, no author or agent signature, no changelog. Version
  and date belong on the COMP's About page ({{ skill("component-checklist") }}).
- **Keep it true.** Update the comment in the same step that changes the node's
  role; delete it when it stops being true. A stale comment is worse than none,
  because the next reader will believe it.

| Weak | Strong |
|------|--------|
| `noise TOP` | `base plate noise — seed is bound to the COMP's Seed par so clones differ` |
| `null` | `terminal out — every Select in this project reads this path; do not rename` |
| `feedback` | `1-frame trail feedback; Reset pulse on the parent COMP clears it` |
| `set by agent 2026-08-30` | `luma key threshold tuned against the stage footage, not a default` |
| `important!` | `blur is before the key on purpose — keying the sharp edge banded` |

## Reading a network with comments

1. `inspect` the parent COMP first. The child roster hands you every child's
   name, opType **and** comment in one call — read that before descending.
2. Descend only into children whose comment (or absence of one) leaves the
   question open.
3. When a comment contradicts the live wiring or parameters, `inspect` wins.
   Fix the comment in the same pass — leaving a known-false comment in place is
   a defect you introduced.
4. Carry the comments into your OpSketch (`# …` trailing form) so the sketch you
   return to the user preserves the intent, not just the topology —
   {{ skill("opsketch-notation") }}.

## Definition of Done

- [ ] Every non-self-evident node created this session carries a `comment`
- [ ] Every COMP hub created or restructured this session carries a `comment`
      naming its role and its In/Out contract
- [ ] First line of each comment stands alone under ~160 chars
- [ ] Comments touched by a role change were updated in the same batch
- [ ] No comment left that the final `inspect` contradicts

## Related

- {{ skill("network-design") }} — in-network documentation and layout conventions
- {{ skill("component-checklist") }} — About page, the COMP-level counterpart
- {{ skill("opsketch-notation") }} — carrying comments into a sketch
- {{ skill("python-api") }} — `n.comment` from a script
- {{ skill("definition-of-done") }} — comments are metadata, not evidence


---

**Canonical:** {{ skill("node-comments") }}
