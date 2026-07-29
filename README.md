# td-mcp-rs

Design notes for a **from-scratch** TouchDesigner MCP, implemented in Rust.
Status: **pre-implementation plan** (not a product yet).

Successor intent relative to the Node fork
`[asyade/touchdesigner-mcp](https://github.com/asyade/touchdesigner-mcp)`
(multi-instance + extras on top of upstream touchdesigner-mcp): keep the
**lessons**, drop the **fork-shaped architecture**.

---

## Motivations (honest)

The Node fork proved multi-instance is necessary and that peer identity,
bridge lifecycle, and agent tool shape matter more than “more CRUD.” It also
showed that bolting hub/lifecycle onto an existing MCP surface accumulates
failure modes (spawn races, dual transports, Node reload vs hub lifetime,
tox/module drift) — including a **sticky-target** session model this design
**rejects**.

### Why Rust (daemon)

Rust is the implementation language because the **control plane** needs a
strong async + OS-facing stack — not because “rewrite = reliability.”


| Need                      | Fit                                                                 |
| ------------------------- | ------------------------------------------------------------------- |
| Powerful async runtime    | **Tokio** — long-lived daemon, many bridges, Streamable HTTP MCP, timers |
| Actor / message model     | **Actix** (or a newer actor crate) — per-pid supervisors, isolation |
| Local transports          | First-class **named pipe**, UDS, HTTP/WS, process I/O               |
| Extended platform support | Same binary story across Windows / macOS (+ Linux later if wanted)  |
| Speak to WinAPI properly  | `windows` / winapi crates — dialogs, process inspect, pipe security |
| Testable abstractions     | Traits over IPC / process / bridge → unit tests without live TD     |


Also: single static binary (no Node runtime for operators), predictable
long-lived process + tray, typed IPC / state machines.


| Rust does **not** buy       |
| --------------------------- |
| Correct peer addressing     |
| TD cook-thread / GIL safety |
| Dialog / freeze handling    |
| “Remote TD” security        |


Reliability comes from a **narrow protocol**, **explicit per-call `pid`**,
and **observable liveness / task queues**. Rust makes the control plane
easier to build and test; it doesn’t replace those contracts.

---

## Goals (v1)

1. **Multi-instance first** — every mutating call names the target by OS
   `pid`. No session “current target,” no generated peer ids.
2. **Live operate only** — inspect, mutate, script, verify on a connected
   bridge. No `.toe` / `.tox` binary editing.
3. **Agent-shaped surface** — small tool set; **`fleet` → `inspect` → `capture`**
   three-layer read model; **perception** is explicit (`capture` tool);
   **uniform diagnostics** (rustc-style errors + lints + mitigation); summary-by-default;
   timeouts fail the *wait* (not claim TD cancelled).
4. **Connected ⇒ usable** — `bridge: "connected"` ⇒ any MCP caller may
   address that `pid`. Coordination via visible tasks + optional exclusive
   requests that fail when the queue is busy.
5. **One local control plane** — long-lived daemon owns pid→bridge map,
   per-pid queues, MCP surface, bridge sessions.
6. **Self-contained delivery** — one binary + one drop-in `.tox` bootstrap.
7. **Resurrection on reconnect** — on IPC loss the daemon **states the
   disconnect** (not a silent heal) and **stacks cancelled tasks** until the
   **first successful task** afterward (then erase).

### Non-goals (v1)

| Item | Why |
| --- | --- |
| Sticky / `select_target` / session peer | Replaced by per-call `pid` + `fleet` |
| Generated `targetId` / UUID / path-hash | **`pid` is the only id** |
| Offline ToeDigest / `.toe` write / inject | Separate MCP; v1 adopt path = drop tox |
| Remote / WAN TD control | Auth, TLS, exposure — after local contract is boring |
| Full dashboard GUI | Tray + status API first |
| Multiple bridge protocols | One local IPC |
| Silent auto-reconnect | Explicit policy below |
| Lifecycle create/start/stop | **P2+**; needs destDir + TD exe resolution |

### Stretch (v1.x)

- System tray: Dashboard / Restart / Stop
- WebSocket MCP if Streamable HTTP is insufficient
- SQLite only when a concrete persistence need appears (see Storage)

---

## Lessons kept (from hub / agent ops)

Design constraints, not nostalgia — **except sticky**, dropped on purpose:

1. **No sticky, no generated peer id.** Address by `pid` every call;
   `fleet` answers “what exists / what’s connected / what’s busy.”
2. **TD dials out** over local IPC; handshake binds the connection to the
   connecting process’s `pid` (OS ground truth).
3. **Daemon lifetime ≠ IDE MCP client lifetime.** Bridges survive IDE MCP
   reconnect if the daemon stays up.
4. **Liveness = IPC connection for that pid.** No `bridge: "connected"` ⇒
   discovery only. On **IPC death**: mark disconnected, cancel waits, stack
   cancelled-task trace. On **process exit**: drop the mapping.
5. **UI dialogs block the world** (Windows `#32770`, thread conflicts). Surface
   them; auto-dismiss only where safe and owned.
6. **Timeouts end the client wait** — TD may still be running. Disconnect
   cancellation is a *separate* explicit status, not a timeout lie.
7. **Token discipline** — `detailLevel: summary` defaults; store-first for
   **perception** captures; tasks/popups/traces on `fleet` only when requested.
8. **No foreign content into a shared lab** — identity and dest ownership
   matter even without inject tools.
9. **Concurrency is a queue, not a mutex in the agent’s head.** Exclusive
   fails if the queue is non-empty (including when only shared tasks exist).
10. **Perception is explicit** — structural reads (`inspect`) and **perception**
    (`capture`) are separate tools. Trigger keyword **perception** routes to
    `capture`; builders never self-grade perception (spawn **perception-critic**).
11. **Diagnostics are curated, not ad hoc** — every failure uses stable `code`
    strings, mitigation steps, and optional corpus/`api_help` references.
    Preflight aggregates independent errors; apply reports partial truth honestly.

---

## Architecture

```text
 IDE (Cursor / …)  ──┐
                     │  MCP Streamable HTTP (localhost; WebSocket optional if needed)
 Other MCP callers ──┤  any caller may address any connected pid
                     ▼
 ┌────────────────── Daemon (Rust) ──────────────────────────────────────┐
 │  MCP adapter  │  pid → bridge  │  per-pid task queue  │  tray?       │
 └──────────┬────────────────────────────────────────────────────────────┘
            │  local IPC (v1): Win named pipe / macOS Unix domain socket
            ▼
   TD process(es)  ←── bootstrap .tox (handshake → FS path → load Python)
```

### Daemon

- Single local process: pid→bridge map, per-pid task queues, MCP adapter,
  bridge controller.
- Loopback only unless remote mode is designed later.
- **Start: auto-upsert** (health → lockfile → spawn) + tray. **Trigger: the
  Cursor plugin / MCP client spawns the daemon on first call** — if the HTTP
  endpoint refuses connection, the plugin launches the binary (`--start`)
  and retries. Manual start stays for debugging.
- Curated lifecycle: no zombies; reclaim lock from stale daemon; graceful
  exit with realistic timeout on in-flight tasks.

### Addressing (replaces sticky + targetId)

| Rule | Detail |
| --- | --- |
| Ground truth | **OS `pid` only** — no UUID, slug, path-hash, daemon-minted id |
| Discovery | `fleet` lists by `pid` + title, window hint, bridge, tasks, traces |
| Usable | `bridge: "connected"` ⇒ any MCP caller may address that `pid` |
| Addressing | Every process-scoped tool takes required `pid: number` |
| No session | Daemon stores no “selected” pid for the IDE |
| IPC loss | Mark disconnected; cancel waits; **stack cancelled-task trace** |
| Resurrection | Same `pid` re-handshakes ⇒ connected again; traces stay until first success |
| Process exit | Pid gone ⇒ drop mapping |
| Pid reuse | Best-effort same-process check (title, image, start time). Mismatch ⇒ **clear state only** |

### Task queue

Per pid, visible in `fleet` when asked:

| Mode | Behavior |
| --- | --- |
| **Shared (default)** | Enqueue / run per daemon policy; agent sees current tasks |
| **Exclusive** | Fails if queue is **non-empty** (any shared or exclusive). Cannot take exclusive while only shared tasks exist. Shared may enqueue behind an in-flight exclusive. No separate “lease that rejects shared.” |

Sole-control flow: `fleet` → pick `pid` → see queue empty → issue
exclusive or wait/backoff. No select dance.

### Disconnect, cancel, resurrection

When the IPC link for a `pid` dies (daemon or tox side), the daemon must
**not** pretend the bridge stayed up:

1. **State the loss** — `bridge` becomes disconnected / `lastDisconnectAt`
  (exact field names TBD); in-flight MCP callers get an explicit
   `bridge_lost` (or equivalent) error — not a generic timeout.
2. **Cancel work** — queued and waiting tasks for that pid are cancelled;
  TD-side work already running may still finish (same honesty rule as
   timeouts), but the *daemon wait* is dead.
3. **Keep a trace** — **stack** cancelled tasks across losses (name,
  exclusive flag, reason=`bridge_lost`, timestamps) so `fleet` can
   explain what died. Do **not** clear on reconnect alone.
4. **Resurrection** — if the **same OS `pid`** completes handshake again,
  mark bridge connected and treat the peer as usable. Disconnect facts and
   the cancelled stack stay visible.
5. **Clear on first success** — the stacked failure trace
  (`cancelledTasks`, `resurrected` / `lastDisconnectAt` as applicable) is
   **erased after the first successful task** for that pid since the
   failure window began. Failures keep stacking until that success. A
   resurrected pid whose first task *fails* keeps the stack — only a
   successful task clears it.
6. **Not resurrection / pid reuse** — the daemon does a **best-effort**
  same-process check (window title, process image/path, start time, and
   similar OS hints — not a hard guarantee). If a numeric pid is alive again
   but is **not** the same TD process, **clear that pid’s prior state only**
   (cancelled stack, resurrected flags, stale bridge attrs) and treat the
   handshake as a fresh peer. Do not resurrect old traces onto a recycled
   pid.

### Surfaces

| Direction | Transport (v1) | Role |
| --- | --- | --- |
| IDE → daemon | **Streamable HTTP MCP** (localhost) | Tool calls; Cursor registers the daemon URL |
| IDE → daemon | WebSocket | Optional later; not required |
| IDE → daemon | stdio MCP | **Not used** for Cursor v1 |
| TD → daemon | **Local IPC** (below) | Bridge; daemon is controller, TD is peer |
| Operator → daemon | Tray / local HTTP status | Human monitor (may share the HTTP listener) |

**Streamable HTTP listen (v1):** bind `127.0.0.1` only; port `9860`
(override via RC / flag); MCP path `/mcp` → `http://127.0.0.1:9860/mcp`;
**no auth** on loopback (non-loopback out of scope).

### Bridge transport (v1) — local IPC

TD connects over **platform local IPC**, not WebSocket or per-instance TCP.

| OS | Mechanism | Address |
| --- | --- | --- |
| Windows | Named pipe (`CreateNamedPipe`) | `\\.\pipe\tdmcp-rs` |
| macOS | Unix domain socket (`AF_UNIX`) | `{dataDir}/bridge.sock` |
| Linux | Unix domain socket (`AF_UNIX`) | `{dataDir}/bridge.sock` |

Fact check (not the same OS primitive):

- Windows named pipes don’t exist on macOS. POSIX FIFOs (`mkfifo`) are a
  weaker, half-duplex tool — **not** the macOS peer.
- Portable pattern: Win named pipe **or** macOS/Linux UDS — one logical
  endpoint, two backends. Rust and Python both split cleanly.
- TD/Python: embedded CPython exposes stdlib. Win: named-pipe client via
  Win32 / `multiprocessing.connection` (`AF_PIPE`). macOS: `socket.AF_UNIX`.
  Cook-thread rules still apply to `op()`; IPC I/O is fine off the cook thread.

Daemon listens; each TD tox **dials out** and announces its `pid` on
handshake. One protocol framing over that byte stream. Stable endpoint name
(not versioned); restrictive perms (pipe ACL / socket `0600`); single-user
v1; version lives in the handshake / manifest, not the name.

### Identity model

One key (`pid`); other fields are attributes of that process:


| Layer             | Examples                                                                   | Use                                                  |
| ----------------- | -------------------------------------------------------------------------- | ---------------------------------------------------- |
| **Key**           | OS `**pid`**                                                               | Sole address for tools and queues                    |
| **Process attrs** | window title, exe/image, start time, responsive/frozen *hint*, `.toe` path | Discovery + **best-effort same-process fingerprint** |
| **Bridge attrs**  | `bridge: connected|disconnected|…`, protocol version, **tasks[]**          | Operability + coordination                           |
| **Loss attrs**    | `lastDisconnectAt`, **cancelledTasks[]** (bridge_lost), resurrected flag   | Honest reconnect / resurrection traces               |


`fleet` must not pretend “process listed” equals “bridge connected.”

---

## MCP tools (v1 sketch)

### Tool definition template

| Field | Content |
| --- | --- |
| **Name** | stable snake_case |
| **Description** | one job; say what it does *not* do |
| **Params** | typed; arrays not CSV; process-scoped tools require `pid`; operator paths use **`OpPath`** (see below) |
| **Defaults** | summary / caps / timeouts; `exclusive?: false` where relevant |
| **Diagnostics** | uniform **`diagnostics`** envelope on every tool response (see below); stable `code` strings; curated mitigation — **no free-string-only failures** |
| **Output shape** | small; `resultRef` for large payloads / full tracebacks |

**Three independent detail flags** (do not overload one):

| Flag | Controls | Values |
| --- | --- | --- |
| `detailLevel` | `fleet` / `inspect` structural detail | `summary` (default) \| `detailed` |
| `diagnosticLevel` | diagnostics payload size: lint caps, raw traceback, full TD error dump | `summary` (default) \| `detailed` |
| `resultRef` | large payloads (images, big node trees, raw tracebacks) always go to a ref | `true` (default for big) \| `false` |

### Three layers: `fleet` → `inspect` → `capture` (perception)

The tool surface is split by **scope**, not by “how much detail.” Names encode
which question you are asking:

| Layer | Tool | Scope | Answers |
| --- | --- | --- | --- |
| **Fleet** | `fleet` | Daemon — all TD processes | *Which* `pid`? *Connected* or discovery-only? *Busy* (tasks)? *Popups*? *Prior disconnect* / cancelled work? |
| **Network (structure)** | `inspect` | One `pid` — one subtree | *What nodes* under a path? *Params*? *Errors*? **No perception by default.** |
| **Network (perception)** | `capture` | One `pid` — one path / face | *What does it look/sound like?* Pixels or basic signal summary. **Trigger keyword: perception.** |

**`fleet` picks the process. `inspect` reads structure. `capture` gathers perception.**

Do **not** merge perception into default `inspect` — cost, timeout (~120s class),
and output shape (MCP image / perception payload) differ from structural reads.
Agents invoke **`capture` when perception is the claim** (look, waveform sanity,
zone face check). Builders never self-grade perception — spawn **perception-critic**
after `capture`.

```text
                    ┌─────────────────────────────────────┐
                    │  fleet  (no pid required)           │
                    │  WHO exists · WHO is connected      │
                    │  WHO is busy · resurrection traces  │
                    └──────────────┬──────────────────────┘
                                   │ pick pid (title / toePath / tasks)
                                   ▼
                    ┌─────────────────────────────────────┐
                    │  inspect  (pid required)              │
                    │  structure: nodes · params · errors │
                    └──────────────┬──────────────────────┘
                                   │ understand / plan edit
                                   ▼
         mutate_nodes · call_node · execute_python
                                   │
                                   ▼
                    ┌─────────────────────────────────────┐
                    │  capture  (pid required) — perception │
                    │  pixels / basic signal · store-first  │
                    └──────────────┬──────────────────────┘
                                   │ perception-critic grades look (if claimed)
                                   ▼
              inspect (errors?) · re-capture (perception?) · fleet (still right pid?)
```

**Typical agent loop**

1. **`fleet`** — default entry. See all processes; filter `include: tasks` when
   coordinating; `include: cancelled` after a suspected drop.
2. **Choose `pid`** — use `title` / `toePath` for human intent, not window title
   alone. Require `bridge: "connected"` before mutate/inspect/capture.
3. **`inspect`** — one structural read for the mutation zone: `path`, `maxDepth`,
   `include`. Summary default; **never perception by default.**
4. **Mutate** — `mutate_nodes` (stacked batch; use `dryRun` to preflight) or
   `execute_python`. On failure, read **`diagnostics`** (codes, lints, mitigation)
   — not raw strings alone. Pass the same `pid` and `contextPath` on every call.
5. **Verify structure** — `inspect` (`include: errors`).
6. **Verify perception** — when look/signal is the claim: **`capture`** (store-first),
   then **perception-critic** PASS/FAIL. Perception claims without `capture` = FAIL.
7. **Re-`fleet` when context may have changed** — not every tool call, but:
   after `bridge_lost`, before exclusive work, when two projects look alike,
   after a long script, or when `queue_busy` / `unknown_pid`.

**Naming rule:** fleet-scoped tools omit `pid` or treat it as optional filter;
network-scoped tools **require** `pid` + usually `path` / `contextPath`.
**`capture` is the sole perception entry point** — trigger keyword **perception**.
Meta tools (`describe_tools`, `api_help`) sit outside all three layers.

### Global harnesses (apply to all tools)

Two cross-cutting contracts govern every tool response: **`OpPath`** (how
operator references are resolved) and **`diagnostics`** (how failures are
reported). Both are defined once here and referenced by the catalogue and
per-tool sections below.

#### Operator references — `OpPath`

All tools that address operators share one reference system — **not invented
by the MCP**, resolved by **TouchDesigner on the bridge** via `td.op()` (or
equivalent TD APIs). The daemon and MCP adapter **never mint parallel path
namespaces** (no `targetId`, no sticky prefix, no Rust-side path algebra).

| Field | Role |
| --- | --- |
| **`OpPath`** | String passed to TD for resolution — absolute or relative |
| **`contextPath?`** | Optional anchor for relative paths on network-scoped tools |

**Resolution contract (bridge Python):**

| Form | Resolution |
| --- | --- |
| Absolute (`/project1/zone/out1`) | `td.op(path)` |
| Relative (`./out1`, `../sibling`, `child`, `a/b/../c`) | Resolve from **`contextPath`** when provided; otherwise default to **project root** (`/project1`) |

**Rules:**

1. **Delegate to TD** — do not normalize or rewrite paths in Rust/Node except
   where TD provides an API. Segments like `..` are valid **only if TD
   resolves them** from the given base (verify in P0 matrix on lab).
2. **Canonical output** — after resolution, tools echo TD’s **`node.path`**
   (absolute) in results so agents store ground truth.
3. **Same contract everywhere** — `inspect.path`, `mutate_nodes` paths,
   `capture.path`, `call_node.path` all use `OpPath` + optional `contextPath`.
   `execute_python` is **exempt by default** (scripts use TD's own `op()`
   resolution); the daemon passes `contextPath` as a Python global
   (`__tdmcp_context_path__`) and a bridge helper `tdmcp_resolve(path)` that
   scripts *may* use but are not required to.
4. **Not the same as parameter expressions** — tool `OpPath` fields address
   **nodes**; par values that contain `op('…')` strings follow TD expression
   rules separately.
5. **Mutation zone** — agents should set `contextPath` to the zone COMP root
   (e.g. `/project1/_agent_scratch/my_fx`) and use `./out1`, `./noise1` inside
   the batch — same relative style as in-network TD authoring.
6. **Default base** — when `contextPath` is omitted, the bridge resolves
   relative paths from **project root** (`/project1`). Deterministic and
   agent-friendly; never the bridge execution context (`me`), which is
   undefined for MCP-driven calls.
7. **Lint carve-out** — *resolution* delegates to TD; *suggestions* may be
   synthesized from inspected nodes (fuzzy match on siblings / subtree). The
   "no parallel path namespace" rule forbids invented *resolution*, not
   invented *hints*.

#### Diagnostics — uniform error handling

Every tool response (success or failure) may carry a **`diagnostics`**
block with the **same shape** — Rust-compiler-inspired, agent-oriented.
Free-string-only failures are **forbidden** for v1 tools; human-readable
`summary` plus structured items.

**Envelope (sketch):**

```json
{
  "ok": false,
  "data": { "results": [], "failedAt": 2 },
  "diagnostics": {
    "summary": "3 errors, 2 lints — batch stopped at step 2",
    "items": [
      {
        "severity": "error",
        "code": "tdmcp.op.not_found",
        "layer": "mutate",
        "message": "Could not resolve OpPath './out1' from context '/project1/my_fx_comp'",
        "span": { "tool": "mutate_nodes", "mutationIndex": 1, "field": "path" },
        "context": { "opPath": "./out1", "contextPath": "/project1/my_fx_comp" },
        "lints": [
          {
            "severity": "lint",
            "code": "tdmcp.op.similar_name",
            "message": "Similar node: '/project1/my_fx_comp/out1_1' (TD auto-rename)",
            "confidence": "high",
            "suggestion": { "opPath": "/project1/my_fx_comp/out1_1" }
          }
        ],
        "mitigation": [
          "Re-inspect the zone with include: nodes",
          "If out1_1 is correct, use canonical path or $ref from create step",
          "Do not replay the whole batch — fix from failedAt only"
        ],
        "references": [
          { "kind": "doc", "id": "op_path_harness" },
          { "kind": "corpus", "id": "corpora/td-software/distilled/python-td-module/README.md" }
        ]
      }
    ]
  }
}
```

**Severity ladder:**

| Severity | Meaning | Agent behavior |
| --- | --- | --- |
| `error` | Step failed / claim blocked | Must fix or change plan |
| `lint` | Hint; did not fail alone | Consider before retry; never auto-apply |
| `note` | Context (partial apply, timeout honesty) | Read; don’t panic |
| `help` | Curated playbook snippet | Follow mitigation |

**`layer` vs `span` — two scopes, not redundant:**

| Field | Granularity | Values |
| --- | --- | --- |
| `layer` | Coarse routing (which agent move family) | `fleet` \| `structure` \| `perception` \| `mutate` \| `script` |
| `span.tool` | Exact tool + step index | `mutate_nodes` + `mutationIndex`, `execute_python` + `line`, … |

`layer` tells the agent *which loop to re-enter*; `span` tells it *exactly where
the failure is*. Both required on every item.

**Errors belong to a layer** — the table below is illustrative; **mitigation
comes from the diagnostic catalog**, not agent memory.

| Code (example) | Layer | Agent move |
| --- | --- | --- |
| `tdmcp.bridge.unknown_pid` | Fleet | Re-`fleet`; pid exited or never connected |
| `tdmcp.bridge.lost` | Fleet | Re-`fleet`; check `cancelledTasks`; wait for resurrection |
| `tdmcp.bridge.queue_busy` | Fleet | Re-`fleet` with `include: tasks`; backoff or wait |
| `tdmcp.bridge.timeout` | Fleet | TD may still be running; re-`fleet` to check liveness, don't assume cancel |
| `tdmcp.td.*` (compile / cook) | Structure | Re-`inspect` on touched subtree; follow mitigation refs |
| `tdmcp.perception.black_frame` | Perception | Re-`inspect` wiring/face; re-`capture`; perception-critic FAIL |
| `tdmcp.perception.no_path` | Perception | Wire zone `out1` / `opviewer`; then `capture` `mode: preview` |
| `tdmcp.op.not_found` | Mutate | Inspect zone; use lint suggestions; fix paths / `$ref` |
| `tdmcp.script.*` | Script | Fix script using span + references; prefer `mutate_nodes` for simple CRUD |

**`tdmcp.bridge.timeout` vs `tdmcp.bridge.lost`** — distinct, do not conflate:
`timeout` = the *daemon wait* ended; TD may still be cooking (re-`fleet` to
check). `lost` = the *IPC link* died; cancel + resurrection stack applies.

**Stable `code` strings** (e.g. `tdmcp.op.not_found`) — agents and skills key
off codes; humans read `message`. Catalog is source of truth for
`message` templates, `mitigation[]`, and `references[]`.

**Where logic lives:**

| Layer | Responsibility |
| --- | --- |
| **Rust daemon** | Transport failures (`tdmcp.bridge.*`), JSON schema, catalog lookup, token caps |
| **Bridge Python** | TD `op()` resolution, fuzzy lints, TD `errors()` enrichment, script traceback parse |
| **`diagnostics/catalog.yaml`** | Versioned with daemon — every code has mitigation + optional corpus / `api_help` refs |

**Token discipline:** default payload = structured summary; full tracebacks,
raw TD error dumps, and large lint lists gated by **`diagnosticLevel: detailed`**.
Large payloads (images, big node trees) always go to `resultRef`. Cap lints
per item (e.g. 3) regardless of level.

**Code families (non-exhaustive):**

| Family | Tools | Examples |
| --- | --- | --- |
| `tdmcp.bridge.*` | all process-scoped | `unknown_pid`, `lost`, `queue_busy`, `timeout` |
| `tdmcp.op.*` | inspect, mutate, capture, call | `not_found`, `outside_zone`, `similar_name` |
| `tdmcp.par.*` | mutate set, call | `unknown`, `type_mismatch` |
| `tdmcp.batch.*` | mutate_nodes | `skipped_dependent` |
| `tdmcp.script.*` | execute_python | `execution_failed`, `unknown_par`, `typo_op_family` |
| `tdmcp.td.*` | inspect errors | `glsl_compile`, `python_dat`, `missing_input` |
| `tdmcp.perception.*` | capture | `black_frame`, `no_path` |
| `tdmcp.dialog.*` | dialogs | `blocked` |

**OpPath resolution failures** — when `td.op(resolved_path)` fails, run a
**bounded** suggestion pass (cap cost):

1. Sibling rename under resolved parent: `name`, `name1`, `name_1`, …
2. Case / separator variants
3. Same basename under `contextPath`, its parent, `/project1` (capped scan)
4. Batch `$ref` miss — step `id` missing or prior step failed

Each lint carries `suggestion.opPath` (canonical TD path) and `confidence:
high|medium|low`. **Never auto-apply** suggestions.

**`inspect` + TD errors** — collect **all** errors in subtree via
`node.errors(recurse=True)` (never first-only). Enrich each line with
classifier code, mitigation, and references — don’t duplicate collection,
upgrade presentation.

**`execute_python` failures** — parse traceback → classify → attach references
(span: line/column/snippet). Examples: unknown par → `api_help` query;
`op()` None → `tdmcp.op.not_found`; GLSL compile → glsl corpus ref.
Raw traceback only in `diagnosticLevel: detailed`.

**Starter catalog (non-exhaustive):**

| Code | Trigger | Mitigation (short) | Lint |
| --- | --- | --- | --- |
| `tdmcp.op.not_found` | OpPath resolution failed | inspect nodes; fix contextPath | `similar_name`, `wrong_base` |
| `tdmcp.op.outside_zone` | path outside mutation zone | use authorized subtree | — |
| `tdmcp.batch.skipped_dependent` | later step skipped after failure | fix `failedAt` only | — |
| `tdmcp.par.unknown` | set unknown par | api_help for OP family | similar par name |
| `tdmcp.script.execution_failed` | exec exception | fix using span + refs | typo / heuristic lints |
| `tdmcp.td.glsl_compile` | TD errors() GLSL | pulse Reinit; inspect shader | glsl corpus ref |
| `tdmcp.perception.black_frame` | capture top black | inspect wiring; re-capture | — |

### Catalogue — simplified patterns

Guiding idea (same move that collapsed `list_targets` + `select_target` +
`get_info` into `fleet`): collapse the Node-fork tool surface into the
fewest agent-shaped calls. Names provisional; not exhaustive — sketches to
pressure, not a spec.

| Tool | Replaces (Node fork) | Job | Key params / notes |
| --- | --- | --- | --- |
| `fleet` | `list_td_targets`, `select_td_target`, `get_td_info` | Fleet + per-pid state (bridge, tasks, popups, cancelled traces) | **No `pid` required**; optional `pids?` filter; `include?`, `detailLevel?`; summary default |
| `execute_python` | `execute_python_script` | Run Python in TD; `result = …` | `pid` req; `resultRef` cache; `exclusive?`; timeout fails the wait, not TD. **OpPath-exempt**; `contextPath` exposed as `__tdmcp_context_path__` + `tdmcp_resolve()` helper (optional) |
| `inspect` | `get_td_nodes` + `get_td_node_parameters` + `get_td_node_errors` | One read for a subtree (nodes + params + errors) | **`pid` req**; `path` (`OpPath`), `contextPath?`, `include: nodes\|params\|errors`, `maxDepth`, `filter`; `includeInfra: false` default |
| `mutate_nodes` | `create_td_node` + `update_td_node_parameters` + `delete_td_node` | **Stacked network mutations** — create, set, delete in one call | **`pid` req**; `contextPath?`; `mutations[]`; `dryRun?`; optional batch `id` / `$ref`; returns **`diagnostics`**; one queue task |
| `call_node` | `exec_node_method` | Call a method on a node (cook, pulse, store, …) | `pid`, `path` (`OpPath`), `contextPath?`, `method`, `args?` |
| `capture` | `get_top_image` (+ future family modes) | **Perception** — pixels or basic signal summary | `pid` req; `path` (`OpPath`), `contextPath?`; `mode: auto\|top\|preview\|chop_data`; `maxSize?`; store-first; trigger **perception** |
| `dialogs` | `td_ui_dialogs` | List / dismiss OS dialogs | `pid`, `action: list\|dismiss`, `id?`; Windows first; best-effort |
| `api_help` | `get_td_classes` + `get_td_class_details` + `get_td_module_help` | One introspection entry for TD Python API — **live introspection** (queries the connected TD) | `pid` req; `query`, `depth?`; returns class/module/help summary |
| `describe_tools` | `describe_td_tools` | Manifest for agents | no `pid`; static |

**Not planned (v1):** `select_target`, `get_info`, sticky getters/setters,
`targetId`, `inject_td_mcp`, `get_toe_digest`, `get_toe_node` — superseded
by `fleet` + per-call `pid`, or deferred to a separate offline MCP.

**Deferred (P2+):** `create_td_project`, `start_td_project`,
`stop_td_project` — lifecycle; returned handle is still the new `pid`.

**Why these collapses (not just fewer names):**

- `inspect` merges three reads that an agent almost always wants together
  (a subtree, its params, its errors). One round-trip; one summary; the
  `include` flag controls cost. Avoids the Node pattern of “get nodes, then
  get params per node, then get errors.”
- `api_help` merges three API-introspection tools into one queryable entry.
  Agents usually want “what is `parGroup` / how do I pulse a parameter” —
  not three separate calls.
- `dialogs` keeps list + dismiss in one tool with an `action` param instead
  of two near-identical tools.
- **`mutate_nodes`** collapses create / set / delete into one batch tool so
  agents can **stack** a network edit plan in a single round-trip (one queue
  task). Explicit `action` per step — no magic upsert. Returns **`diagnostics`**
  with preflight aggregate + apply-phase partial truth. `execute_python`
  stays the workhorse for wiring, one-offs, and anything the batch schema
  does not cover.
- **`mutate_nodes` vs `execute_python` decision rule:** use `mutate_nodes` for
  structured create/set/delete (low token, diagnostics, preflight). Use
  `execute_python` for wiring, conditional logic, family-specific ops not in
  the batch schema, or anything needing `td.` globals beyond `op()`. When both
  could work, prefer `mutate_nodes` for the diagnostics + queue shape.
- **Diagnostics** replace td-mcp’s free-string errors — rustc-style items with
  lints (e.g. similar node names), mitigation, and corpus references.
- **`capture`** is the single **perception** entry point — replaces `capture_top`
  and future chop/pop perception tools. **`inspect` never returns pixels by default.**
  Say *perception* → call `capture`.

### `mutate_nodes` — stacked network mutations

**Job:** Apply an ordered list of network mutations on one `pid` in a single
MCP call. Replaces separate `create_node` / `set_node` / `delete_node` tools.

**Does not:** read structure (`inspect`), perception (`capture`), or call
methods (`call_node`). No magic upsert — each step has an explicit **`action`**.

| `action` | Phase | Fields (sketch) |
| --- | --- | --- |
| `create` | **P1** | `parent` (`OpPath`), `type`, `name?`, `params?`, optional `id` |
| `set` | **P1** | `path` (`OpPath` or `$ref`), `params` (partial) |
| `delete` | **P1** | `path` (`OpPath` or `$ref`) |
| `wire` | **P1.x** | `from`, `to`, `input?` — deferred; use `execute_python` until then |

**Params (sketch):**

```json
{
  "pid": 34,
  "exclusive": true,
  "contextPath": "/project1/my_fx_comp",
  "mutations": [
    { "action": "create", "id": "out", "parent": ".", "type": "nullTOP", "name": "out1" },
    { "action": "set", "path": "$out", "params": { "label": "out" } },
    { "action": "delete", "path": "./scratch_res" }
  ]
}
```

**Batch semantics — two phases:**

| Phase | When | Behavior |
| --- | --- | --- |
| **Preflight** | `dryRun: true`, or implicit validation before apply | **Aggregate all** independent OpPath / schema failures; **zero mutations applied** |
| **Apply** | default mutate | Sequential steps; **stop at first hard dependency failure**; later dependent steps → `tdmcp.batch.skipped_dependent` **notes**, not errors |

| Rule | Detail |
| --- | --- |
| **Sequential apply** | Steps run in array order on the bridge (apply phase only) |
| **Not atomic** | TD has no transactions — partial apply is possible |
| **One queue task** | Entire batch = one per-pid task (fits exclusive semantics) |
| **Per-step results** | `results[]` with `index`, `action`, `ok`, canonical `path`, linked diagnostic codes |
| **Preflight aggregate** | e.g. 5 bad OpPaths → 5 errors + lints in one response, nothing mutated |
| **Apply stop** | Step N fails → stop; steps N+1 that depend on N become `skipped_dependent` notes |
| **Batch-local refs** | Optional `id` on a step; later steps use `"$id"` — MCP batch scope only. **`id` binds to the canonical `node.path` TD returns after `create` resolves**, so `$ref` survives TD auto-rename (`out1` → `out1_1`). |
| **Caps** | Max mutations per call (e.g. 32); max params per `set`; max lints per error |
| **Batch timeout** | Separate tier from script timeout: base (e.g. 10s) + per-step (e.g. 2s), capped. A long exclusive batch must not starve the queue silently — timeout ends the *wait*, batch state is reported honestly. |
| **Zone policy** | `tdmcp.op.outside_zone` when path leaves authorized mutation zone |

**After partial failure:** read `diagnostics` + `failedAt` + `results[]`; re-`inspect`.
Do **not** replay successful create steps. Mitigation in catalog says “fix from
failedAt only.”

**Optional later:** `action: ensure` — narrow idempotent zone scaffolding only.
**`dryRun: true`** — full preflight (P1; may ship simplified in P0 for OpPath tests).

### `capture` — perception tool (progressive)

**Trigger keyword:** `perception`. When an agent or skill mentions *perception*,
*look*, *visual verify*, or *see the output*, route to **`capture`** — not
`inspect`.

**Job:** Gather **basic perception** from a connected `pid`: cooked output
as pixels (preferred) or capped signal summary. Store-first; long timeout;
temp converters always destroyed. Does **not** replace `inspect` for structure
or errors.

**Not in scope for `capture`:** full TD app / Network Editor screenshot (OS
bitmap). Perception means **network truth** — terminal TOP, Operator Viewer
face, or family-specific conversion to pixels/data.

| `mode` | Phase | Behavior |
| --- | --- | --- |
| `top` | **P0** | TOP → JPEG via `saveByteArray` (+ optional temp `resolutionTOP`); black frame = perception fail |
| `preview` | **P0** | Resolve zone **presentation path** with fallback chain: COMP `opviewer` par → terminal TOP (`./out1`) → any direct TOP child → clear `tdmcp.perception.no_path` error. Preferred “what the face shows”. |
| `auto` | **P0** | TOP → `top`; COMP → resolve `preview`; else clear error (“no perception path”) |
| `chop_data` | **P1** | CHOP → capped JSON (channel names, rate, min/max, last N samples) — **basic perception**, not image |
| `pop` | **P1.x** | Temp `poptoTOP` → `top` → destroy temp |
| `chop_image` | **P1.x** | Temp trail/chopto → TOP → `top` → destroy temp |
| `render` / OS screenshot | **Defer** | Scene-dependent Render TOP; WinAPI window grab — not v1 |

**Params (sketch):** `pid` (req), `path`, `mode?: auto|top|preview|chop_data|…`,
`maxSize?`, `storeFirst?: true`, caps for CHOP sample windows.

**Perception DoD (basic, v1):**

1. `capture` returns stored artifact path + short observation text
2. TOP/preview: non-black frame (same honesty as Node `get_top_image`)
3. Perception **claims** require **perception-critic** PASS — builder must not
   self-grade from `capture` summary alone
4. Re-`capture` only when the touched subtree / face changed (store-first reuse)

**Family → perception path (reference):**

| Hero family | Basic perception path |
| --- | --- |
| TOP | `mode: top` on path |
| COMP (zone face) | `mode: preview` — `opviewer` → `./out1` |
| CHOP | `chop_data` (P1); image only via converter (P1.x) |
| POP | temp `poptoTOP` → `top` (P1.x) |
| SOP / Geo | defer — needs Render TOP convention |
| DAT | text summary in `inspect`; visual only via Text TOP (defer) |

### Example: `fleet`

- **Name:** `fleet`
- **Job:** Fleet view — TD processes by `pid`, bridge status, optional
popups/tasks, and **disconnect / cancelled-task traces** after a loss or
resurrection. This is how the agent picks a `pid`, sees whether exclusive
work can run, and notices that a reconnect cancelled prior work.
- **Params:**
  - `pids?: number[]` — optional filter (JSON array, not CSV)
  - `include?: ("process" \| "bridge" \| "popups" \| "tasks" \| "cancelled")[]`
  — default `["process","bridge"]` (include `"tasks"` / `"cancelled"` when
  coordinating or after a suspected drop)
  - `detailLevel?: "summary" \| "detailed"` — default `summary`
- **Output (summary):** compact list keyed by `pid`; no sticky / targetId.

```json
{
  "processes": [
    {
      "pid": 33,
      "title": "TouchDesigner 2025.31760: C:/opened_project.toe",
      "windowStatus": "responsive",
      "toePath": "C:/opened_project.toe",
      "bridge": "connected",
      "tasks": [
        { "name": "PythonEval", "exclusive": false }
      ]
    },
    {
      "pid": 34,
      "title": "TouchDesigner 2025.31760: C:/opened_project_2.toe",
      "windowStatus": "responsive",
      "toePath": "C:/opened_project_2.toe",
      "bridge": "connected",
      "resurrected": true,
      "lastDisconnectAt": "2026-07-29T19:51:02Z",
      "tasks": [],
      "cancelledTasks": [
        {
          "name": "PythonEval",
          "exclusive": true,
          "reason": "bridge_lost",
          "cancelledAt": "2026-07-29T19:51:02Z"
        }
      ]
    },
    {
      "pid": 35,
      "title": "TouchDesigner 2025.31760: C:/opened_project_3.toe",
      "windowStatus": "frozen",
      "bridge": "disconnected",
      "lastDisconnectAt": "2026-07-29T19:40:00Z",
      "cancelledTasks": [
        {
          "name": "GetTopImage",
          "exclusive": false,
          "reason": "bridge_lost",
          "cancelledAt": "2026-07-29T19:40:00Z"
        }
      ]
    }
  ]
}
```

Popup detail (when `include` asks for it): title/message when available,
dismiss actions if supported — still OS-fragile; treat as best-effort.

Process-scoped example shape (any mutate/script tool):

```json
{
  "pid": 34,
  "exclusive": true,
  "script": "result = {'ok': True}"
}
```

If `exclusive: true` and pid 34’s queue is not empty → error `queue_busy`
(agent re-`fleet`s or retries later).

---

## TouchDesigner side

### Operator UX

Drop a **single bootstrap `.tox`** into the project (no manual module tree copy).

### Bootstrap contract

1. Tox knows only how to reach the daemon handshake over **local IPC**.
2. Handshake returns a **filesystem path to a bridge package directory** the
   daemon owns — not inline code, not a download URL. Local-only v1.
3. Tox loads that package from disk (entry module in the dir) and runs it;
   that code owns the IPC session and RPC.
4. On disconnect / daemon death / re-handshake: **reload from the path the
   new handshake returns.** No remembered path, no code cache, no “last good
   payload.” Every connect is a fresh FS load.

### Bridge package on disk

```text
bridge/
  manifest.json   # protocolVersion, minDaemon, entry, checksum…
  __init__.py      # or entry named in manifest
  …               # supporting modules
```

Package directory (not a lone `.py`) so it can grow without handshake churn.
`protocolVersion` / `minDaemon` live in **`manifest.json`** — a sidecar the
daemon can read without importing Python; module attrs may mirror them but
the manifest is source of truth.

### Security note (v1 local)

v1 ships **no code over the wire**. Trust boundary = daemon process + the
files it exposes under its install/data dir. Harden later for remote (signed
payloads); don’t invent crypto for a same-machine path hand-off.

### Delivery details (concrete)

**Daemon auto-upsert (Cursor plugin):** plugin config registers the HTTP URL
(`http://127.0.0.1:9860/mcp`). On first MCP call, if the connection refuses,
the plugin launches the daemon binary with `--start` (working dir = data dir,
stdout/stderr to a log file), waits briefly, and retries. Spawn contract:
`tdmcp-rs --start [--port 9860] [--data-dir …]`. Stale lockfile (pid not
alive) → reclaim and start; live daemon → connect.

**`diagnostics/catalog.yaml`:** loaded from disk beside the daemon at startup;
versioned with the daemon. CI gate: every `code` referenced in bridge/daemon
source has a catalog entry (message template, mitigation, references). If the
file is missing at startup, daemon falls back to a baked-in minimal catalog
and emits a `note` diagnostic — never hard-fails on catalog absence.

**Bridge package versioning:** `manifest.json` carries `protocolVersion` +
`minDaemon`. Daemon checks `minDaemon` ≤ its own version before loading; a
bridge below `minDaemon` is rejected with a clear `tdmcp.bridge.version`
diagnostic. Bridge may also check daemon version and refuse if too old.
Daemon is controller; bridge is loaded fresh every handshake.

**Bootstrap `.tox` adopt path:** for a running TD, operator drops the tox into
the project; it dials the named pipe / UDS, handshakes, loads the bridge
package from the returned FS path, and starts the session. For new projects,
same flow. No restart of TD required; the tox is the only manual step.

**`OpPath` in `execute_python`:** daemon passes `contextPath` as Python global
`__tdmcp_context_path__` and exposes `tdmcp_resolve(path)` helper in the script
namespace. Scripts *may* use them but are not required to — raw `op('./out1')`
still works using TD's own resolution from the script's `me`. Documented,
optional, no enforcement.

---

## Storage

OS-conventional data dir (safe to delete = wipe settings/state):

| OS | Path |
| --- | --- |
| Windows | `%LOCALAPPDATA%/tdmcp-rs/` |
| macOS | `~/Library/Application Support/tdmcp-rs/` |
| Linux | `$XDG_DATA_HOME/tdmcp-rs/` or `~/.local/share/tdmcp-rs/` |

### RC file

Human-editable: default TD version / install dir(s), default projects
directory, daemon bind / Streamable HTTP listen, feature flags (tray, WS).

### SQLite

Don’t invent a schema in the abstract. Add SQLite only when something durable
needs queryable history (audit, metrics, logs). Until then: JSON state keyed
by `pid` for bridges / queues — no sticky, no targetId.

---

## Delivery

- Sibling repo **`td-mcp-rs`** (this tree), not under creative-corpus.
  Packaging, releases, MCP plugin wiring live here.
- One Rust binary for the daemon (+ tray GUI).
- Bridge Python package + `manifest.json` + **`diagnostics/catalog.yaml`**
  beside the daemon; handshake only tells TD the directory path.
- Bootstrap `.tox` distributed with releases (tiny: dial IPC + load path).
- TD still runs Python inside the peer — “no external deps” means **no Node
  for the operator machine**, not “no Python in TD.”
- **Implementation status:** Cargo workspace under `crates/` — see
  [`ARCHITECTURE.md`](ARCHITECTURE.md), [`CONSTITUTION.md`](CONSTITUTION.md),
  [`AGENTS.md`](AGENTS.md). Local gate: `scripts/check.ps1` / `scripts/check.sh`
  (`cargo fmt --check` + `cargo clippy -- -D warnings` + `cargo test`).
  **P0 control plane wired:** daemon runs an `IpcListener` accept loop; each
  handshaken TD peer gets a per-pid actor (`tdmcp-daemon/src/bridge.rs`) that
  owns the `IpcStream`, drives queue progression (`start_next` / `complete_task`),
  and answers `BridgeRpc::call`. `dispatch_tool` (async) enqueues eagerly
  (exclusive-while-busy checked at enqueue) then calls the bridge with a 30s
  wait budget; `execute_python` / `inspect` / `capture` map to the uniform
  `diagnostics` envelope (`tdmcp.script.execution_failed`,
  `tdmcp.perception.*`, `tdmcp.bridge.*`). Integration tests drive the real
  actor over a memory IPC pair (`tests/bridge_session.rs`): handshake, fleet,
  execute_python, capture, exclusive-while-busy, disconnect → resurrection →
  first-success-clears-stack. **Real MCP transport wired:** `tdmcp-mcp::McpHandler`
  implements `rmcp::ServerHandler` over the same `AppState`/`dispatch_tool`
  path the JSON fallback uses; the daemon nests `rmcp`'s `StreamableHttpService`
  (`LocalSessionManager`, legacy session mode) at `/mcp/rpc` alongside the
  JSON `/mcp/tools/list` + `/mcp/tools/call` surface (kept as a low-ceremony
  fallback for curl/tests). Tool schemas are hand-authored per tool
  (`rmcp_handler.rs`); bridge failures map to `CallToolResult::structured_error`
  with the same `tdmcp.*` diagnostics envelope. Integration tests
  (`tests/rmcp_streamable_http.rs`) drive the real transport end-to-end over a
  live TCP listener: `initialize` → `notifications/initialized` → `tools/call`
  for `fleet` / `execute_python` (happy path, script failure, unknown tool).
  **Still pending for P0 exit-green:** the live-TD run of
  `docs/E2E_CHECKLIST.md` (bootstrap `.tox` + bridge IPC dial are code-complete
  but not yet exercised against a running TouchDesigner instance).

---

## Phased delivery


| Phase  | Ship                                                                                                                                                                                         | Exit green                                                                                                                                                        |
| ------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **P0** | Daemon + local IPC bridge + bootstrap tox + **Streamable HTTP MCP**: `fleet` + script/errors + **`capture` basic perception** (`top`, `preview`) + **diagnostic envelope** (bridge/script/perception codes) with required `pid` + per-pid task queue + exclusive fail + disconnect/resurrection traces | Two connected pids; exclusive fails while busy; resurrection traces; **perception** on zone `out1` via `preview` non-black; script failure returns structured diagnostic + reference; bridge errors use `tdmcp.bridge.*` codes |
| **P1** | **`mutate_nodes`** (create/set/delete batch + preflight), **`capture` `chop_data`**, dialogs (Win), **op lint engine** | Agent builds network in one `mutate_nodes`; preflight with 3 bad paths returns **3 aggregated errors** + ≥1 `similar_name` lint; partial apply emits `skipped_dependent` notes |
| **P1.x** | **`capture` `pop`, `chop_image`** | Non-TOP heroes get intentional pixel path via temp converters |
| **P2** | Tray + status UI + **lifecycle** create/start/stop (return `pid`)                                                                                                                            | Operator can restart/stop without killing from Task Manager; new project addressable by pid                                   |
| **P3** | WebSocket MCP if needed; remote story RFC                                                                                                                                                    | Separate design review — not bolted on                                                                                                                            |


---

## Decided (v1 contract)

- **TD↔daemon transport:** local IPC — Win named pipe `\\.\pipe\tdmcp-rs`,
  macOS/Linux UDS `{dataDir}/bridge.sock`. Not WS/TCP-per-instance; not FIFOs.
- **Handshake:** returns a local FS path to a **bridge package directory**;
  TD reloads from disk on every handshake (no path memory, no code cache).
  No wire download / inline Python.
- **Disconnect / resurrection:** state the loss, cancel waits, stack
  cancelled tasks; same `pid` re-handshake ⇒ usable again; **stack erased on
  first successful task**. Process exit drops the mapping.
- **Pid reuse:** daemon best-effort same-process check; mismatch ⇒ clear
  that pid’s state only (no resurrect onto a recycled pid).
- **Cursor↔daemon MCP:** **Streamable HTTP on localhost** — `http://127.0.0.1:9860/mcp` (port overridable), loopback only, no auth. No stdio. WS optional later.
- **Daemon start:** **auto-upsert** + tray; manual for debug only.
- **Exclusive:** fail iff queue non-empty (any shared or exclusive); cannot
  take exclusive while only shared tasks exist; shared may enqueue behind
  exclusive.
- **Lifecycle create/start/stop:** **P2+**, not v1; returns `pid`.
- **Bridge on disk:** package directory + `manifest.json`
  (`protocolVersion`, `minDaemon`, entry, …).
- **Long-term home:** sibling repo **`td-mcp-rs`** (this tree), not under
  creative-corpus.
- **Perception tool:** **`capture`** (trigger keyword **perception**); separate
  from `inspect`. P0 = `top` + `preview`; P1 = `chop_data`; P1.x = `pop` /
  `chop_image`. Builders never self-grade perception — **perception-critic**.
- **Network mutate:** **`mutate_nodes`** — batched `create` / `set` / `delete`;
  replaces separate CRUD tools. Explicit actions; no magic upsert. One call =
  one queue task.
- **Operator paths:** **`OpPath`** + optional **`contextPath`** on all
  network-scoped tools; TD `op()` resolution on bridge; canonical `node.path`
  in outputs. No MCP-invented path namespace. **Default base = project root**
  (`/project1`) when `contextPath` omitted. `execute_python` exempt by default;
  `contextPath` exposed as `__tdmcp_context_path__` + `tdmcp_resolve()` helper.
- **Daemon start:** **Cursor plugin / MCP client** spawns the daemon on first
  call (connection refused → `--start` → retry); auto-upsert for stale daemon;
  manual for debug.
- **Detail flags:** three independent — `detailLevel` (fleet/inspect structure),
  `diagnosticLevel` (error/lint payload), `resultRef` (large payloads always).
- **Diagnostics:** uniform **`diagnostics`** envelope on all tool responses;
  stable `code` strings; **`diagnostics/catalog.yaml`** for mitigation +
  references; severities `error|lint|note|help`; **`diagnosticLevel`** gates
  payload size (separate from `detailLevel`). Free-string-only failures
  forbidden. Preflight aggregates; apply stops on dependency failure with
  `skipped_dependent` notes.

*(No open contract decisions for the v1 surface. Implementation details —
field names, caps, lab-verified TD `..` path behavior, `ensure`/`wire`/`continueOnError`
— are tracked in phases, not open contract questions.)*

---

## Challenge log

### First pass


| Axis                         | Finding                                                                                                   | Doc change                                                                            |
| ---------------------------- | --------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| Overcomplexity               | “Various protocols,” remote TD, full GUI, SQLite-with-no-schema, one-shot-only reconnect                  | Cut to one tunnel; defer remote/GUI/SQLite; define reconnect states                   |
| Executability / architecture | Daemon+MCP+tray as one blob risks coupled restarts; manual-only start fights agent UX                     | Split surfaces in diagram; prefer auto-upsert; phase tray                             |
| Verbosity / info level       | Goals were slogans; tools unfinished; empty SQLite/RC                                                     | Concrete goals/non-goals, tool groups, phases, open decisions                         |
| Risky / underexplored        | Remote code exec into TD; window-title identity; HTTP MCP client support; no inject path for foreign toes | Security note; identity layers; HTTP MCP for Cursor; manual tox drop as v1 adopt path |
| Testing / deployment         | “Extremely reliable” with no gates                                                                        | Phased exit-green table                                                               |
| Scope                        | Toe MCP + remote + dashboard + multi-protocol + Rust rewrite all at once                                  | P0–P3; toe editing stays out                                                          |
| Docs / skills                | creative-operator still points at Node td-mcp                                                             | Deferred until P1 works; then dual-run / cutover plan                                 |


### Second pass (sticky removal)


| Axis                         | Finding                                                                                | Doc change                                                            |
| ---------------------------- | -------------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| Overcomplexity               | Sticky + `select_target` + `get_info` duplicated `fleet` and invented session state | Removed; per-call address; connected ⇒ any caller                     |
| Executability / architecture | Multi-caller / multi-agent needs coordination without a single sticky owner            | Per-pid task queue + exclusive fail-if-busy                           |
| Risky / underexplored        | Two agents can still race on shared (non-exclusive) ops                                | Documented: exclusive fails if queue non-empty; shared may enqueue        |
| Docs / skills                | Operator skills today teach sticky workflow                                            | Cutover must rewrite `cop-touchdesigner-mcp` sticky sections          |


### Third pass (pid-only identity)


| Axis                  | Finding                                                               | Doc change                                                          |
| --------------------- | --------------------------------------------------------------------- | ------------------------------------------------------------------- |
| Overcomplexity        | Parallel `targetId` namespace (UUID/path hash) duplicated OS identity | Removed; `**pid` is the only id**                                   |
| Risky / underexplored | OS pid reuse after exit can mis-route if mapping is stale             | Drop on exit; best-effort fingerprint; mismatch ⇒ clear state only  |
| Docs / skills         | Skills/tools still say `targetId` / sticky                            | Address everything as `pid`                                         |


### Fourth pass (bridge = local IPC)


| Axis           | Finding                                                                                          | Doc change                                                       |
| -------------- | ------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------- |
| Fact check     | “Named pipe on Win+Mac” is false if taken literally; Mac peer is UDS, not Win pipe / not FIFO    | Document Win named pipe + macOS `AF_UNIX`; one framing over both |
| Executability  | TD Python can client both (stdlib / Win32); cook-thread rules still apply to `op()`, not IPC I/O | Lock as v1 TD↔daemon transport; drop WS/TCP-per-instance from P0 |
| Open decisions | Handshake “URL vs pipe” was still open                                                           | Decided local IPC + fixed `tdmcp-rs` pipe / `bridge.sock`             |


### Fifth pass (handshake = FS path)


| Axis           | Finding                                                        | Doc change                                                               |
| -------------- | -------------------------------------------------------------- | ------------------------------------------------------------------------ |
| Overcomplexity | Inline/download Python + asymmetric crypto for same-machine v1 | Handshake returns path only; TD FS-loads; crypto deferred to remote mode |
| Reliability    | Cached path/code drifts from daemon upgrades                   | Reload every handshake; no path memory; no code cache                    |
| Open decisions | “Beside binary vs always download”                             | Decided package dir + `manifest.json`; leftover cleared                  |


### Sixth pass (resurrection)


| Axis                   | Finding                                                                | Doc change                                                                 |
| ---------------------- | ---------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| Honesty                | Silent reconnect hides cancelled work and looks like a successful wait | State disconnect; `bridge_lost` cancels; keep `cancelledTasks` trace       |
| Addressing vs liveness | “Drop mapping on any IPC death” fought same-pid reconnect              | IPC loss ≠ process exit; resurrection for same pid; reuse mismatch ⇒ clear state only |
| Open decisions         | How long to keep cancel traces                                         | **Stack until first success**, then erase; not TTL / ack / reconnect-clear |


### Seventh pass (tool catalogue + doc pressure)


| Axis                  | Finding                                                                                          | Doc change                                                                 |
| --------------------- | ------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------- |
| Overcomplexity         | Node fork ships ~14 live tools; many are read-triples or near-duplicates                        | Collapsed to 9: `inspect` merges 3 reads; `mutate_nodes` merges 3 CRUD; `api_help` merges 3; `dialogs` merges 2 |
| Executability          | Per-node `get_td_node_parameters` + `get_td_node_errors` round-trips blow tokens                 | `inspect` returns subtree + params + errors in one summary; `include` gates cost |
| Risky / underexplored  | Node CRUD tools can drift from TD’s real op surface (families, par types, expressions)          | `mutate_nodes` for structured batches; `execute_python` for wiring and gaps; not exhaustive TD coverage |
| Risky / underexplored  | `fleet` vs `inspect` both sounded like generic “look at things”          | Renamed fleet tool; documented two-layer scope split + error routing        |
| Risky / underexplored  | `fleet` summary could hide a resurrected pid from a coordinating agent   | Summary always shows `resurrected` + `cancelledTasks` count when non-empty; full detail via `include` |
| Verbosity / info level | “Decided” list repeated each point as a paragraph                                                 | Compressed to one bulleted contract block                                  |
| Scope                  | Doc still mixes v1 contract with stretch (tray, WS, SQLite) inline                              | Stretch isolated; v1 contract lives in one “Decided” block                  |
| Docs / skills          | Catalogue names are provisional but skills will lock them early                                   | Flag names as provisional; do not update `cop-*` skills until P0 exits green |


### Eighth pass (perception / `capture`)


| Axis | Finding | Doc change |
| --- | --- | --- |
| Overcomplexity | `capture_top` + future chop/pop tools = sprawl | Single **`capture`** tool; **perception** trigger keyword |
| Executability | Merging pixels into `inspect` blows tokens/timeouts | Three layers: `fleet` / `inspect` / **`capture` (perception)** |
| Scope | Full family auto-capture too big for P0 | Progressive `mode` table; P0 = `top` + `preview` only |
| Docs / skills | Align with perception-critic | Basic perception DoD; builder never self-grades perception |


### Ninth pass (`mutate_nodes` + `OpPath`)


| Axis | Finding | Doc change |
| --- | --- | --- |
| Overcomplexity | Three CRUD tools + N round-trips for one network plan | Single **`mutate_nodes`** with `mutations[]`; `delete` via `action: delete` |
| Executability | Agents need ordered stacks (create → set → delete) | Two-phase batch: preflight aggregate + apply stop; `$ref`; `diagnostics` per step |
| Risky / underexplored | Invented path rules would diverge from TD | Global **`OpPath`** harness: TD `op()` on bridge; `contextPath` for relative; canonical `node.path` out |
| Risky / underexplored | `./out1` is context-relative in TD, not “project root” | Document `contextPath` as zone anchor; verify `..` segments on lab in P0 |
| Scope | Wire/connect in batch is P1.x | `action: wire` deferred; `execute_python` until then |
| Docs / skills | Uniform refs must apply to inspect/capture/call too | `OpPath` section applies to all network-scoped tools |


### Tenth pass (diagnostics harness)


| Axis | Finding | Doc change |
| --- | --- | --- |
| Overcomplexity | td-mcp free-string errors give agents no playbook | Uniform **`diagnostics`** envelope; catalog-driven mitigation |
| Executability | “Fail-fast” hid independent batch errors | **Preflight aggregate** vs **apply stop**; `skipped_dependent` notes |
| Executability | Node-not-found needs fuzzy hints | **`tdmcp.op.similar_name`** lint (bounded search); never auto-apply |
| Risky / underexplored | Rich diagnostics blow MCP tokens | `detailLevel: summary\|detailed`; cap lints; `resultRef` for raw traceback |
| Risky / underexplored | Script tracebacks opaque to agents | Parse → classify → `references[]` (corpus, api_help); span line/snippet |
| Scope | inspect already aggregates TD errors | Enrich TD `errors()` output; don’t re-invent collection |
| Testing | No gate for diagnostic quality | P1 exit: preflight 3 bad paths → 3 errors + lint; catalog CI for every code |


### Eleventh pass (re-pass: conflicts, deepening, reorg)


| Axis | Finding | Doc change |
| --- | --- | --- |
| Overcomplexity / coherence | "No open decisions" was false — caps, field names, `..` paths, `ensure`/`wire` deferred | Reframed: no open *contract* decisions; impl details tracked in phases |
| Overcomplexity | `detailLevel` overloaded 3 concerns (structure, diagnostics, resultRef) | Split into `detailLevel` + `diagnosticLevel` + always-`resultRef` for big |
| Risky / underexplored | `contextPath` default = bridge `me` (undefined for MCP calls) | Default to **project root** (`/project1`); deterministic |
| Risky / underexplored | "No path algebra" vs lint suggestion engine | Carve-out: *resolution* delegates to TD; *suggestions* may be synthesized |
| Executability | `execute_python` claimed same OpPath contract — scripts use raw `op()` | Exempt by default; `contextPath` as Python global + optional helper |
| Executability | Daemon auto-upsert trigger undefined | Cursor plugin spawns on first call (`--start`); documented spawn contract |
| Executability | `mutate_nodes` exclusive + no batch timeout tier | Separate batch timeout (base + per-step); queue-starvation honesty |
| Executability | `$ref` after TD auto-rename undefined | `id` binds to canonical `node.path` post-create; `$ref` survives rename |
| Executability | `capture` preview brittle on `opviewer` only | Fallback chain: `opviewer` → `./out1` → any TOP child → error |
| Coherence | `layer` vs `span.tool` looked redundant | Defined: `layer` = coarse routing; `span` = exact tool + step |
| Coherence | `tdmcp.bridge.timeout` vs `lost` conflatable | Explicit distinction in catalog + layer table |
| Scope | `api_help` pid requirement ambiguous | Decided: live introspection → `pid` required |
| Docs / skills | Delivery details (catalog loading, bridge versioning, tox adopt) thin | Added "Delivery details (concrete)" subsection |


### Critique of original claims (short)

- **“Start from scratch because Rust”** — scratch is justified by architecture
  debt; Rust by the daemon’s control-plane needs (Tokio, actors, OS-facing
  transports, WinAPI, testable traits). Rewrite without narrowing protocol
  = same pain in a new language.
- **“Remove toe/tox edition”** — correct split; v1 = drop tox, later =
  dedicated digest/inject MCP.
- **“Handshake returns MCP code”** — rejected for v1. Local-only: handshake
  returns a **filesystem path**; TD reloads every handshake. Wire download /
  signed payload is a remote-mode problem, not v1 crypto theater.
- **Sticky session / generated peer ids** — rejected; `pid` is ground truth.
- **Silent reconnect** — rejected; resurrection states the loss, stacks
  cancelled tasks, erases on first success.
- **stdio MCP for Cursor** — rejected; Streamable HTTP on localhost.
- **Pid reuse** — best-effort same-process check; mismatch ⇒ clear state only.
- **Master/slave** — controller/peer language in public docs.
- **“Language is secondary”** — half-true: protocol/`pid`/queues own
  correctness; Rust owns implementability + testability. Keep both claims.

---

## Agent expectations (while planning)

When iterating this doc before implementation:

- Challenge claims; prefer scope cuts over feature lists
- Fact-check against real TD/MCP constraints (cook thread, dialogs, queues)
- Flag incoherence (e.g. “no reconnect” vs “extremely reliable”)
- Detect overcomplexity early; push work into phases with observable exits

