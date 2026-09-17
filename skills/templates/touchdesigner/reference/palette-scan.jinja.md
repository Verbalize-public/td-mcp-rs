# Palette scan and describe

Build the palette knowledge base: scan the roster, probe components in a live
TD, and write the cards that make {{ skill("palette") }} useful — a resumable
loop you can run over a slice of the palette at a time.

## Why this is a deliberate pass

A fresh index knows only **names and categories**. The `summary` and the card
that let you pick a component without loading it are *authored by you*, from
probe evidence. Nothing generates them — the tools produce evidence and store
what you write.

The builtin palette is large (hundreds of components). Divide the requested
scope into bounded, resumable slices, including when the user requests the
whole library. Keep completed IDs and remaining work explicit.

## The loop

**1. Get a TD to probe in.** Never probe in the user's real project — a probe
loads arbitrary components. Spawn a throwaway:

```json
{"projectPath": "<scratch>/palette_probe.toe", "createIfMissing": true}
```

`spawn_td` creates it from the shipped template. Depth: {{ skill("lifecycle") }}.
Confirm the returned PID is connected. Run a small canary before scaling:
verify load, digest and cleanup on this installation. Under Wine, verify the
actual runner/prefix and TD-visible path; do not assume one machine's drive
mapping applies everywhere. A failed canary is a reason to pause and diagnose,
not to blacklist the whole library.

**2. Scan the roster** — offline, no `pid`:

```json
{"action": "scan"}
```

Returns `{roots, added, updated, removed, stale, total, ignored}`. Re-run it
any time; it reconciles against disk and never discards cards.

**3. Pick a slice.** Every bulk action takes the same selector:

| Field | Effect |
| --- | --- |
| `ids` | Exact ids (bypasses the blacklist — asking is deliberate) |
| `category` | `"Tools"` also matches `Tools/Sub` |
| `source` | `builtin` or `user` |
| `match` | `*` / `?` glob over the id |
| `status` | `all` (default) · `undescribed` · `described` · `stale` · `failed` · `ignored` |
| `includeIgnored` | Include blacklisted entries (default: exclude) |

**4. Probe a small batch** — bridged, needs `pid`:

```json
{"pid": 1234, "select": {"category": "Tools", "status": "undescribed"}, "limit": 3}
```

Each component is loaded into a scratch COMP, digested, and destroyed. Keep
batches small: a batch is one bridge call, so a component that wedges TD takes
the whole batch with it.

Each digest describes the component's **interface**: `opType`, `customPars`
grouped by page, `inputs`/`outputs` (its In/Out operators), `childCount`, and
`extensions`. Two fields are worth calling out:

- **`help`** — the text of the component's own `help` DAT, when it ships one.
  This is Derivative's description in their words; lead your card with it rather
  than paraphrasing from node names.
- **`wrapped: true`** — the `.tox` was a palette wrapper (icon + help + the real
  component) and the digest describes the component inside, not the shell. Your
  own `.tox` files come back without it.

`detailLevel: "detailed"` adds `children` and `extensions`; `summary` keeps the
interface and drops the internals, which is usually what you want — a card
describes how to *drive* a component, not how it is built.

`thumbnails: true` also renders a 256px PNG per component into the store's
`thumbs/` folder — Derivative's own icon art for stock wrappers, a viewer
rasterization for unwrapped `.tox` files. It serves the GUI's palette browser,
not your cards, so skip it unless a human is going to look at the roster; each
rendered picture costs a little bridge time inside the same load → digest →
destroy window. A black or uniform frame is reported, not stored.

**5. Write the card** from the digest, one `describe` per component:

```json
{"action": "describe", "paletteId": "builtin:Tools/particlesGpu",
 "summary": "GPU particle system; source TOP in, rendered particles out.",
 "tags": ["particles", "gpu", "instancing"],
 "body": "…"}
```

**6. Repeat** from step 4. The same selection returns the *next* batch each
time, because describing a component drops it out of `status:"undescribed"` —
so re-list the selected slice to verify that its remaining set shrinks.
Global `stats` is a cross-check, not proof that a particular slice is complete.
For forced regeneration of already-described entries or an all-status selection,
freeze an explicit ID roster, partition it and track completed IDs. Such a
selector does not shrink when cards are written; repeatedly taking the first
batch can process the same components forever.

Read each stored card back after a write timeout before retrying: the write may
have persisted. Check summary/body and fingerprint separately from probe status;
an old failed probe flag can coexist with useful content. A repaired probe path
does not prove every requested card was regenerated.

A probe never returns an unexplained empty result. If nothing ran you get a
`note` saying why — everything matched was blacklisted, or nothing matched at
all — and naming an id that is not in the index fails outright rather than
quietly skipping it.

## What a good card looks like

`summary` is the retrieval surface — it is what `list` shows and what a future
agent scans. One line, what it *does*, in the words someone would search for.
Never "a component for particles"; write "GPU particle system driven by a
source TOP".

The `body` is read only when someone has already narrowed to this component, so
make it decision-grade:

````markdown
**What:** one paragraph — what it does, how it works at a glance.
**When:** the situation that should make you reach for it; and when not to.

**Pins:** `in1` TOP source · `out1` TOP rendered result
**Key pars:** `Birthrate` float · `Life` float sec · `Reset` pulse · `Emitter` menu(point/surface)

```opsketch
scope: particlesGpu (COMP:baseCOMP, pars: Birthrate, Life, Reset) nodes=…
…
```

**Gotchas:** cooks every frame even when idle; Reset needed after changing Emitter.
````

Transcribe only what the digest contains: interface, available node roster and
opTypes. Include wires/comments only if actually returned or independently
inspected; an interface digest alone cannot establish internal wiring.
Grammar: {{ skill("opsketch-notation") }};
what earns a `{}` block: {{ skill("opsketch-importance-gating") }}.
Sketch the component's **interface and shape**, not every internal node; if it
has 200 children, sketch the COMP-level outline only.

## The blacklist — how bulk runs survive

Loading a `.tox` runs its startup scripts. Some stock components open network
sockets, look for absent hardware, or raise a modal — any of which can wedge TD
in the middle of a run.

Three layers protect the loop:

1. **Seeded defaults.** `[palette].ignore` ships with the known-hostile
   families (`TDAbleton`, `TDBitwig`, `TDSynchro`, `TDVR`, `MetaQuest`, `Vive`,
   `WebRTC`) already excluded.
2. **Your own entries.** `{"action":"ignore", "patterns":["builtin:Techniques/SICK/*"]}`
   — ids or globs. `unignore` reverses it. Probe skips them and reports
   `{status:"skipped", reason:"ignored"}`.
3. **Auto-ignore.** An entry that fails to probe twice ignores itself, so the
   next bulk run does not re-hit it.

Blacklisted entries are hidden from `list` and `forget` by default — pass
`select.includeIgnored: true` to see or remove them. `unignore` with an exact id
also clears an auto-ignore and its strike count.

### Recovering from a wedge

The ids of an in-flight batch are recorded before dispatch, so the suspect set
is recoverable after TD dies:

1. Calls stall → `dialogs` to see if a modal is blocking ({{ skill("popups") }}).
2. Still wedged → stop new probes; use {{ skill("lifecycle") }} to stop only
   the task-owned throwaway PID. Other agents sharing it must stop too.
3. `palette_index` `{action:"list", select:{status:"failed"}}` — the batch's
   entries come back marked `suspect`.
4. Quarantine the suspect batch with `{action:"ignore", patterns:["<the id>"]}`
   as appropriate; preserve its IDs and evidence before resuming on a healthy
   owned process. A suspect batch does not identify one proven culprit.

Do not retry a suspect component without a new diagnosis. Distinguish a
component-specific failure from a shared loader/path/environment failure before
expanding the blacklist. Exact-ID selectors bypass exclusions: review them
before dispatch, rather than accidentally retrying quarantined work.

On stop/cancel, stop dispatching, account for in-flight work and preserve the
completed, remaining and suspect IDs. Report cleanup as unverified if the
process cannot answer; a terminal tool/job status alone does not prove the
scratch COMP was removed. Parallel helpers may author from collected digests;
live work on one daemon/PID remains sequential ({{ skill("tooling-concurrency") }}).

## Adding your own components

1. Save the `.tox` into the user palette folder (`[palette].user_root`, or TD's
   default `{documents}/Derivative/Palette`), in a category subfolder.
2. `{"action":"scan"}` — it appears as `user:{Category}/{Name}`.
3. Probe and `describe` it like any other.

A component you wrote deserves a *better* card than a stock one: you know the
gotchas. Check it against {{ skill("component-checklist") }} before describing
it — a component without In/Out operators or an About page is worth fixing
first.

## Staleness

Cards are fingerprinted against the `.tox` they were written from. When the
file changes, `scan` marks the card `stale` and `list` shows it — the card is
still served, but treat it as a hint. Re-probe and re-`describe` to clear it.

## Definition of Done

- [ ] Probed in a throwaway project, never the user's work
- [ ] The scratch COMP is gone — `inspect` on `/` shows no probe leftovers
- [ ] Every card's `summary` reads as a search hit, not a category label
- [ ] Every OpSketch transcribed from the digest, nothing invented
- [ ] Suspect components quarantined; shared environment failures distinguished
- [ ] Selected roster reconciled against stored cards; remaining work reported

## Related

- Using what you built: {{ skill("palette") }}
- Spawning the throwaway project: {{ skill("lifecycle") }}
- Clearing a modal that blocks a probe: {{ skill("popups") }}
- Writing the sketch in a card: {{ skill("opsketch-notation") }}
- Judging your own components before describing them: {{ skill("component-checklist") }}

---

**Canonical:** {{ skill("palette-scan") }}
