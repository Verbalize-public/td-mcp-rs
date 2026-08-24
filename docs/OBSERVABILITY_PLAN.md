# Observability & Logging — Implementation Plan

Companion to [`OBSERVABILITY.md`](OBSERVABILITY.md) (spec v2). This document is
the curated, task-level execution plan: exact files, signatures, schemas,
wireframes, test matrices, and acceptance gates. Statuses mirror house style
(**Planned** until shipped).

Conventions used below:

- Line refs are against the tree as of spec v2 (workspace version `0.1.3`).
- "DoD" = definition of done per task; every task ends in a surface observation
  (test run, captured output, screenshot), never "looks fine".
- Rust snippets are sketches — final code passes
  `cargo clippy --workspace --all-targets -- -D warnings` (no
  `unwrap`/`expect`/`panic!` in lib paths, per
  [`CONSTITUTION.md`](../CONSTITUTION.md)).

---

## 1. Milestone graph

```
M1 Sink ──► M2 Uplink ──┬─► M3 TD Mirror ──┐
                        ├─► M4 GUI+API ────┼─► M6 Hygiene ─► release 0.2.0
                        └─► M5 Proxy Ingest┘
```

M3/M4/M5 are independent once M2 lands. M6 last (audit over final code).

| Milestone | Size | Owns | Status |
| --- | --- | --- | --- |
| M1 Sink | L | File sink, ring, retention, subscriber rewrite, CLI tail | **Shipped** |
| M2 Uplink | M | Python logtap, IPC event arm, daemon ingest | **Shipped** |
| M3 TD Mirror | M | Textport tee wiring into TD, face LOGS upgrade, `td.errors` | **Planned** — T3.1 live-verify gate not yet run |
| M4 GUI+API | M | `/admin/logs*`, tray Logs view UX | **Shipped** — GUI not pixel-verified (no live TD/GUI env available at implementation time) |
| M5 Proxy Ingest | S | stdio proxy forwarding | **Shipped** |
| M6 Hygiene | M | Message audit, silent-crate baselines, docs | **Shipped** |

---

## 2. M1 — Central sink (critical path)

### T1.1 Dependencies

`Cargo.toml` (workspace):

```toml
# line 71-72 region becomes:
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt", "registry"] }
tracing-appender = "0.2"   # resolve >= 0.2.3 for max_log_files; assert in Cargo.lock review
```

Why `registry`: workspace pins `features = [...]` which **disables defaults**;
layered `Registry` needs the explicit feature. Why no `json` feature: the sink
serializes records itself (T1.4) — one formatter, shared by file + ring.

Crates adding deps: `tdmcp-daemon` (appender, subscriber already there),
nothing else.

**DoD:** `cargo tree -p tdmcp-daemon -i tracing-appender` shows ≥ 0.2.3.

### T1.2 Config surface — `[logging]`

Seven-touchpoint checklist (all required in this task):

1. **Struct** `crates/tdmcp-config/src/lib.rs` — new section alongside existing
   sections:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingSection {
    /// Override directory; None => `{data_dir}/logs`.
    pub dir: Option<PathBuf>,
    /// EnvFilter string for the file layer; None => RUST_LOG => built-in default.
    pub filter: Option<String>,
    /// Rotated files kept (daily rotation).
    pub max_files: u32,          // default 14
    /// Startup+periodic sweep threshold in days.
    pub retention_days: u32,     // default 30
    /// Separate EnvFilter for the stderr fmt layer; None => current defaults.
    pub console_level: Option<String>,
}
```

2. **Template** `crates/tdmcp-config/assets/default.toml` — new `[logging]`
   section after `[daemon]`, commented-out overrides only (`dir`, `filter`),
   documented defaults for `max_files` / `retention_days`. Same comment tone as
   existing sections (see `default.toml:42-54`).
3. **Resolution** `crates/tdmcp-daemon/src/config.rs` — `Config` gains
   `logging_dir: PathBuf` (resolved `overrides > file > data_dir.join("logs")`,
   following the `catalog_path` pattern at `config.rs:96-99`), plus
   `logging_filter: Option<String>`, `logging_max_files: u32`,
   `logging_retention_days: u32`.
4. **GUI draft**: none in v1 (spec C12) — verify Settings save round-trips the
   TOML without dropping unknown sections (regression test below).
5. **Validation**: `max_files >= 1`, `retention_days >= 1` clamp in resolver.
6. **Tests**: `crates/tdmcp-config` round-trip test asserting `[logging]`
   survives load→save; daemon config test asserting default
   `logging_dir == data_dir/logs`.
7. **Docs**: [`CONFIG.md`](CONFIG.md) gains a `### [logging]` field table
   (shipped with M6 docs pass or immediately — either acceptable, tracked).

### T1.3 Record model — `crates/tdmcp-daemon/src/logrecord.rs` (new)

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub seq: u64,
    pub ts: String,          // RFC3339 UTC ms (chrono, existing dep)
    pub level: Level,        // serde lowercase: trace|debug|info|warn|error
    pub src: Src,            // daemon|ipc|mcp|proxy|gui|bridge  (serde lowercase)
    pub pid: u32,
    pub target: String,
    pub msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kvs: BTreeMap<String, String>,
}

/// One JSON line per record; lossless inverse of the schema in spec §5.0.
pub fn to_line(r: &Record) -> String;      // serde_json to_string + '\n'
pub fn from_line(line: &str) -> Option<Record>; // tolerant: unknown fields ignored
```

`Src` inference rule (documented in-module): target starts with `tdmcp_ipc` →
`ipc`, `tdmcp_mcp` → `mcp`, `tdmcp_gui` → `gui`; everything else in-process →
`daemon`. Bridge/proxy records arrive pre-stamped (M2/M5), never inferred.

### T1.4 Ring — `crates/tdmcp-daemon/src/logring.rs` (new)

```rust
pub struct LogRing { /* Mutex<VecDeque<Arc<Record>>>, cap 2048, seq: AtomicU64 */ }
impl LogRing {
    pub fn push(&self, r: Record) -> Arc<Record>;
    pub fn snapshot_after(&self, after: u64, limit: usize,
        min_level: Option<Level>, srcs: &[Src]) -> (Vec<Arc<Record>>, u64);
    pub fn path_hint(&self) -> usize; // len, for status badge later
}
```

Semantics: `seq` monotonic across eviction; `snapshot_after` returns records
with `seq > after` oldest-first plus the highest seq observed (cursor even when
empty). Filters applied server-side; `limit` clamped to ≤ 512 by callers.

### T1.5 Subscriber rewrite — `crates/tdmcp-daemon/src/tracing_init.rs`

Signature change (only call site is `main.rs:184`):

```rust
pub struct LogHandles {
    pub ring: Arc<LogRing>,
    // tracing_appender::non_blocking::WorkerGuard — held until shutdown so
    // buffered lines flush (drop order matters; keep alive in main).
    pub guard: WorkerGuard,
}
pub fn init(cfg: &Config) -> Result<LogHandles>;
```

Layer stack:

```
Registry
 ├── SinkLayer            (custom, this task)
 │     filter: EnvFilter::try_new(cfg.logging_filter)
 │              .or_else(RUST_LOG)
 │              .unwrap_or("info,tdmcp_daemon=debug")
 │     on_event: Visit{kvs} -> Record{src inferred, pid=std::process::id()}
 │               -> ring.push + file.write_all(to_line(..))
 ├── fmt layer             (unchanged unless [logging].console_level set)
 │     .with_env_filter(cfg.logging.console_level or current default chain)
 │     .with_target(true).with_writer(std::io::stderr)
```

File layer plumbing:
`RollingFileAppender::builder().rotation(Rotation::DAILY)
.filename_prefix("daemon").filename_suffix("log")
.max_log_files(cfg.logging_max_files as usize).build(cfg.logging_dir)` →
wrapped in `non_blocking(...)`.

**Decision recorded here (resolves spec open-question 1):** `TDMCP_LOG` is
**purged**, not implemented — the doc-comment lie goes away, `RUST_LOG` +
`[logging].filter` cover the need. No alias surface.

**DoD (surface-observed):**
- Windows + macOS CI or local: `cargo run -p tdmcp-daemon -- start` creates
  `{data_dir}/logs/daemon.<date>.log` containing valid JSONL (each line
  `from_line(..).is_some()`).
- Kill -9 + restart appends to same-day file; second day rolls (fake clock via
  unit test on rotation naming only).
- Console output byte-comparable to today's stderr format.

### T1.6 Retention sweeps

`sweep_logs(dir, retention_days) -> usize` in `logring.rs` or sibling
`sweep.rs`: deletes `*.log*` older than cutoff **and** any legacy
`daemon.log` in `data_dir` root (one-time migration). Scheduled: once after
`init`, then `tokio::spawn` interval 24 h, cancelled with the shutdown token.

Unit test: temp dir, backdated mtimes (Unix `filetime`-equivalent via
std `fs` — set through platform helper already used in tests or accept
creation-date heuristic on Windows: skip-if-unable, documented).

### T1.7 Delete the fd-attach path

- `crates/tdmcp-daemon/src/ensure.rs`: remove
  `configure_detached_spawn_with_log` (:234), `attach_unix_daemon_log`
  (:270-286); `configure_detached_spawn` (:229) absorbs the semantics (null
  stdio both platforms — file sink makes capture unnecessary).
- Callers: `ensure.rs:327` → `configure_detached_spawn`;
  `admin.rs:257` → same; drop the now-unused `Some(&args.data_dir)` args.
- Doc comments :222-231 rewritten (why: central sink supersedes fd capture).
- Tests referencing `daemon.log` attachment updated/removed.

### T1.8 CLI tail — `tdmcp-daemon logs [N]`

New clap variant in `main.rs` `Commands` (near `:140`): `Logs { n: usize =
50 }`. Reads the newest `daemon.*.log` under `logging_dir`, renders aligned
text: `HH:MM:SS.SSS LEVEL  SRC     TARGET  message {kvs…}`. Errors clearly when
no dir/files. Human-facing complement to JSONL (spec challenge C4).

---

## 3. M2 — Bridge uplink (Python → daemon)

### T2.1 Python module `bridge/tdmcp_bridge/logtap.py` (new)

```python
_LOG_QUEUE_MAX = 256          # lines before drop-oldest
_BATCH_LINES = 32
_BATCH_INTERVAL_S = 0.5

def install(on_flush) -> bool
    """Replace sys.stdout/sys.stderr with Tee instances calling through to the
    originals; re-install-safe (idempotent). Returns True when installed."""
def suppress() -> contextlib.AbstractContextManager
    """During execute_python's own capture window the global tee stands down,
    avoiding double-capture (execute.py swaps streams itself)."""
def append_local(line: str, level: str = "info") -> None
    """Direct entry used by internal bridge prints ('tdmcp-rs: ...')."""
def maybe_flush(force: bool = False) -> None
    """Drain buffer -> on_flush(records) when >=_BATCH_LINES or >=interval."""
class Tee(io.TextIOBase):  # write-through, thread-safe via lock, never raises
```

Rules baked into the module (mirrors `execute.py:47` discipline): a failing
DAT write or flush never propagates; original stream always written first.

### T2.2 Wiring points

- `bootstrap.py`: after successful daemon link, `pkg` calls
  `logtap.install(sender)` where `sender(records)` enqueues an IPC
  `Message::Event{name:"log", payload:{records:[…]}}` onto the existing framed
  writer owned by the connection loop (`tdmcp_bridge/__init__.py:150`).
- Heartbeat path re-asserts installation: compare `sys.stdout is tee_instance`;
  reinstall if TD swapped it.
- `task_queue._pump()` (`task_queue.py:385`): add `logtap.maybe_flush()` next
  to the existing reschedule (:406) — piggybacks the 50 ms tick, no timer
  thread (spec C-batch decision).
- Internal prints (`tox_callbacks.py`, `bootstrap.py`, `task_queue.py`) migrate
  from bare `print("tdmcp-rs: …")` to `logtap.append_local(msg, level)` —
  **keep the Textport print** (append_local still writes through), losing
  nothing, gaining timestamps + levels + uplink.

### T2.3 Rust side — event arm (the hard requirement)

Every `stream.recv_message()` site in `crates/tdmcp-daemon/src/bridge.rs`
gains the same arm:

```rust
Ok(Ok(Message::Event { name, payload })) if name == "log" => {
    ingest_bridge_logs(pid, payload, &sink);   // stamps src:"bridge", pid
    continue;                                   // NOT Disconnected
}
Ok(Ok(Message::Event { .. })) => { /* unknown event: debug! + continue */ }
```

Sites: `await_matching_response` (:481-530, replacing the fatal
`Ok(Ok(_other))` at :517-523 for the log case only — *other* unexpected frames
keep today's disconnect semantics), the idle-dead read loop (~:377 region), and
any future reader (grep gate in review checklist). Events received anywhere
reset the idle-dead clock (they prove liveness).

`ingest_bridge_logs` lives in `logring.rs`: validates each entry (cap 64 KiB
msg, stringify kvs, clamp batch ≤ 256), pushes with daemon-assigned `seq`/`ts`
arrival time (Python `ts` preserved in `kvs.sentTs` for skew diagnosis).

**Regression test (blocking acceptance)**: fake TD peer sends a slow tool
request then interleaves 100 `log` events before responding; assert reply
still matches, all events persisted, zero disconnect warns. Pattern:
`tests/admin_auth.rs` harness (:110-133) with a scripted peer socket.

### T2.4 Payload contract example

```json
{"type":"event","name":"log","payload":{"records":[
  {"level":"info","target":"bridge::user","msg":"render done","kvs":{"ms":"42"}}
]}}
```

Daemon fills: `seq`, `ts`, `pid` (handshake identity, `bridge.rs:671-690` —
never trusted from payload), `src:"bridge"`, `code:null`.

---

## 4. M3 — TD mirror & face LOGS

### T3.1 Live-verify gate (before coding)

Run in real TD via the `touchdesigner` skill operate path; record results in
this file's appendix:

| Probe | Question |
| --- | --- |
| V1 | Does replacing `sys.stdout` capture `print` from an unrelated DAT/node? |
| V2 | Does `debug("x")` route through `sys.stdout` (captured?) or straight to Textport? |
| V3 | Does TD restore `sys.stdout` on save / reload / edit? When? |
| V4 | `iter(td.errors)` cost at 5 s cadence on a mid-size project? |
| V5 | Max face LOGS lines fitting `_FACE_W×_FACE_H=560×560` @ `_FONT_SIZE=10` (`tox_callbacks.py:30-37`) |

V2 outcome decides scope: uncapturable → documented limitation, lean harder on
`td.errors`.

### T3.2 Stream ownership coordination with `execute_python`

`execute.py` swaps `sys.stdout` to `_TeeStream(buf, previous)` during exec
(:35-49) and restores afterwards. With the global tee installed, "previous" is
the global tee → lines would reach both execute's DAT append (:92-106) *and*
the tap. Fix: `execute.py:_append_debug_dat` delegates to
`logtap.append_local` when the tap is installed (single DAT writer, single
uplink path); legacy direct-DAT path kept only for tap-less fallback. Wrapped
in `with logtap.suppress():` around the swap window.

### T3.3 `td.errors` polling

Heartbeat cadence (5 s, `default.toml:64`) not pump cadence. Dedupe LRU keyed
`(op_path, text)` size 500. Each new error →
`logtap.append_local(f"{path}: {text}", level="error",
target="td_errors")` → flows to face + uplink free of charge.

### T3.4 Face LOGS upgrade (`tox_callbacks.py`)

- `_LOG_PANEL_LINES` 14 → value from V5 (target ≤ 22).
- `_debug_log_lines` (:139-148) renders each tail line as
  `HH:MM:SS L msg` (`L` ∈ `I W E D`), clipped by the existing
  `_line(ln[:width], width)` mechanism (:406).
- `( no logs )` idle marker kept (:400-403).
- Refresh cadence unchanged (phase transitions + pump completion, :649/:683) —
  no per-line TOP writes; worst-case staleness = heartbeat period, acceptable
  for a glanceable panel.

### T3.5 Tests & E2E

- Python unit: `tests/test_logtap.py` — tee write-through, drop-oldest,
  batch triggers, suppress() scoping, idempotent install. Runs in CI (pure
  stdlib mocks, pattern of `tests/test_execute_logs.py`).
- Live rows appended to [`E2E_CHECKLIST.md`](E2E_CHECKLIST.md):
  print-from-unrelated-node appears in face ≤ 1 s; broken-node traceback shows
  as `error`; agent `execute_python` still returns `logs` identically
  (S3b/S3c parity re-run).

---

## 5. M4 — Admin API + tray Logs view

### T4.1 Endpoints (`crates/tdmcp-daemon/src/admin.rs`)

Router additions at :67-76:

```rust
.route("/admin/logs", get(admin_logs))          // ?after&limit&level&src
.route("/admin/logs/path", get(admin_logs_path))
```

`AdminState` gains `logs: Arc<LogRing>` (wired in `main.rs` where
`build_admin_router` is called). Responses camelCase, matching `StatusBody`
conventions (:78-99):

```json
{"records":[{"seq":41,"ts":"2026-01-01T12:00:00.123Z","level":"warn",
             "src":"bridge","pid":12345,"target":"bridge::tox_callbacks",
             "msg":"heartbeat pong timeout","kvs":{}}],
 "next":57}
```

Errors: `400` on bad cursor/filter, `503` when ring unavailable (headless
builds still serve — ring exists regardless of GUI).

Auth: `middleware.rs` `requires_psk_auth` (:55-57) adds prefix rule
`path.starts_with("/admin/logs")` covering all three routes (incl. M5 ingest).
Update the unit tests at :169-178 accordingly.

### T4.2 Tray UI/UX (380×600 constraint)

Entry: ghost button `≡` ("Logs") inserted left of `⚙` in the RTL action row
(`lib.rs:913-936`). Active state uses ACCENT tint like Settings' gear.
`View::Logs` variant added at :34; switch helpers mirror `open_settings`
(:413)/back-to-Fleet (:450).

Wireframe (380 px column):

```
┌──────────────────────────────────────────┐
│ td-mcp-rs v0.2.0          ■  ↻  .tox ≡ ⚙ │  ← top bar (existing)
├──────────────────────────────────────────┤
│ LOGS                       [Open folder] │  ← section_header row + reveal
│ (ALL)(ERR)(WRN) · (DAEMON)(BRIDGE)(PROXY)│  ← filter chip row, 12px height
├──────────────────────────────────────────┤
│ ● 14:02:11 D inspect fleet ok            │  ← row: dot·time·src·msg
│ ● 14:02:09 B heartbeat pong timeout      │     ERR dot #ERR, WRN #WARN,
│ ● 14:01:58 D bridge session started      │     INFO TEXT, DBG TEXT_FAINT
│ ▸ …expanded row (selected)               │
│    target bridge::tox_callbacks          │     click toggles expansion:
│    code null · kvs {}                    │     target/code/kvs/meta lines
├──────────────────────────────────────────┤
│ ⏸ Paused  ·  128 shown  ○ FOLLOW (dot on)│  ← footer bar
└──────────────────────────────────────────┘
```

Interaction contract:

| Concern | Behavior |
| --- | --- |
| Rendering | `egui::ScrollArea::vertical` + `show_rows`, `font_mono()`, one `Label` per record, `.truncate()`; expansion is an indented sub-block, not a modal |
| Follow mode | Default ON. New records auto-scroll only if user is at bottom (within 4 px) — reading history never fights the tail. Footer toggle `○ FOLLOW / ● FOLLOW` |
| Pause | Pause freezes fetches (not rendering); badge "Paused" replaces count. Resume fetches with stored cursor — no gap |
| Polling | Only when `view==Logs && visible && !paused`: fetch `?after=next&limit=512` on the existing 250 ms repaint tick (`lib.rs:2362`); fetches naturally throttled by response availability, mirroring the 2 s status throttle (:2352) |
| Filters | Chips toggle `min_level` (ALL/ERR/WRN) and src set; server-side via query params; changing filters resets cursor and refetches tail |
| Empty state | Centered `( no logs )` in TEXT_FAINT — mirrors face convention (:400) |
| Daemon unreachable | Dim inline banner `daemon unreachable — retrying` above footer; keeps retrying; no error dialog |
| Copy | Click-expanded row selects; Ctrl+C copies rendered line. Context menu "Copy record" |
| Open folder | Ghost button → `rfd`/existing reveal helper used by `.tox` action (:925-930) targeting `logging_dir` |
| Keyboard | `f` follow, `space` pause, `esc` back to Fleet (when focused) — registered via egui input, documented in hover tooltips |

State added to the app struct (near `fleet_panel`, :225-226):
`logs_view: LogsViewState { buf: VecDeque<Arc<Rendered>>, next: u64, follow,
paused, min_level, srcs: EnumSet, expanded: Option<u64>, fetch_error }`.
Rendering caps: keep ≤ 2048 rendered rows locally, evict oldest (matches ring).

**DoD (pixel-observed)**: screenshots of empty/following/filtered/expanded/
unreachable states at 380×600; a scripted bridge flood renders smoothly
(no frame drops > 2× baseline in manual check); headless build unaffected.

### T4.3 GUI↔API types

Hand-written structs in `lib.rs` (serde camelCase, mirroring T4.1) — no codegen;
a round-trip unit test pins the contract against a fixture JSON.

---

## 6. M5 — Stdio proxy ingest

- Proxy side (`crates/tdmcp-mcp/src/stdio_proxy.rs`): tiny tracing `Layer`
  feeding an mpsc; flusher task POSTs batches
  (`{lines:[Record…]}` ≤ 64 KiB) to `/admin/logs/ingest` every 500 ms with the
  same bearer handling as tool calls. Fire-and-forget: failures drop the batch
  silently after one `eprintln` per minute max (stderr still owned by Cursor).
- Daemon side: `POST /admin/logs/ingest` handler → `Record` validation →
  `ring.push` + file write with `src:"proxy"`, pid stamped from
  `std::process::id()` of the *daemon* is wrong here — stamp `src:"proxy"`
  with `pid:0` and carry proxy pid in `kvs.proxyPid` (loopback peer is trusted
  enough for a display hint only).
- Acceptance: with daemon running, one MCP tool call through the proxy yields
  `src:"proxy"` lines centrally; daemon stopped → tool call still succeeds.

---

## 7. M6 — Message hygiene audit

Procedure per crate (grep-driven, then human pass):

1. Census all macro sites (`rg "\b(info|warn|error|debug)!\("` ).
2. Apply spec §5.7 rules; record each change in the PR description table:
   file:line / before / after / rule violated.
3. Silent-crate baselines added: `tdmcp-core` registry connect/drop (info),
   `tdmcp-config` override application (debug), per spec.
4. Delete: duplicate retry-loop warns (keep first + final), success-path info
   noise, `"stdio_proxy:"`-style prefixes (superseded by `src`), stale claims.
5. Extend completeness test ([`TESTING.md`](TESTING.md) item 6) to scan log
   literals for `code:"…"` values against `diagnostics/catalog.yaml`.

Docs touchpoints closing the effort: [`CONFIG.md`](CONFIG.md) `[logging]`
table, [`CONTRACT.md`](CONTRACT.md) observability row (catalogue §), 
[`E2E_CHECKLIST.md`](E2E_CHECKLIST.md) M3/M4 rows, README feature bullet,
repo `AGENTS.md` docs-reference table row for `OBSERVABILITY.md`.

Release: workspace version `0.1.3 → 0.2.0` (root `Cargo.toml:25`); rebuild +
restart flow per repo `AGENTS.md` (kill daemons → build → `ensure`).

---

## 8. Test matrix (consolidated)

| Axis | Test | Milestone |
| --- | --- | --- |
| Schema | line↔Record round-trip; unknown-field tolerance | M1 |
| Filter | EnvFilter precedence: config > RUST_LOG > default | M1 |
| Rotation | daily naming; max_files prune | M1 |
| Sweep | age predicate; legacy `daemon.log` removal | M1 |
| Config | 7-section round-trip incl. `[logging]` | M1 |
| Uplink | interleaved-events regression (blocking) | M2 |
| Uplink | drop-oldest + `dropped=N` marker under flood | M2 |
| Mirror | logtap unit suite (CI, mocked td) | M3 |
| Mirror | execute parity: S3b/S3c unchanged | M3 |
| API | cursor resume loses nothing; limit clamp; auth 401 under psk | M4 |
| GUI | pixel states (manual, screenshotted) | M4 |
| Proxy | central arrival; offline resilience | M5 |
| Hygiene | code-literal catalog completeness scan | M6 |

## 9. Risk register (plan-level deltas)

| Risk | Mitigation |
| --- | --- |
| `tracing-appender` `max_log_files` regresses below 0.2.3 in lockstep updates | Pin note in Cargo.toml comment + T1.1 DoD check |
| Custom SinkLayer drops field-visitor edge cases (Error sources, nested %sigils) | Visitor covers `RecordArgs/RecordFields` + `%`/`?` sigil normalization; fuzz-ish unit test with exotic fields |
| Global tee interacts badly with third-party TOXes that also swap stdout | Write-through guarantees originals still work; tap reinstall is identity-checked, never fights (replaces only when `sys.stdout` is not already our tee) |
| GUI fetch storm when ring churns fast | Server `limit≤512`, client renders ≤2048 rows, fetch piggybacks existing tick — bounded by construction |
| Scope creep into metrics/tracing export | Non-goal list in spec §3; reviewers reject OTLP-shaped PRs until a new spec revision |

## 10. Estimation & sequencing

| Milestone | Rough effort | Parallelizable with |
| --- | --- | --- |
| M1 | 2–3 d | — (everything waits on it) |
| M2 | 1–2 d | — |
| M3 | 2–3 d (incl. live-TD probes) | M4, M5 |
| M4 | 2 d | M3, M5 |
| M5 | 0.5–1 d | M3, M4 |
| M6 | 1–1.5 d | after M2–M5 merge |

Total ≈ 9–13 working days single-threaded; ≈ 6–8 with M3/M4/M5 overlapped.
