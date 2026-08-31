# Payload Spool Plan — bounded payloads + artifact file delivery

Status: **plan only — nothing implemented.**

Context: the limits pass already raised caps (32 MiB IPC frame, 4 MiB script
and result payloads) and fixed the silent-loss bugs, but two ceilings are
immovable — the 32 MiB IPC frame and the 16 MiB rmcp SSE event — and
cap-raising can never remove the context cost of shipping megabytes of
base64 through a model conversation. This plan closes that class properly:
small payloads stay inline forever; large ones spill to daemon-managed temp
files and travel as references.

## 1. Problem statement — why base64 is structural here

Every byte TD sends rides **one JSON pipeline**; binary has no native lane:

```
capture.py:227   raw = source.saveByteArray(".png")          # real bytes exist here
capture.py:244   "imageBase64": base64.b64encode(raw)...     # +33%, becomes a Python str
transport.py:82  body = json.dumps(msg)                      # full re-encode (copy #1)
framing.rs       MAX_FRAME = 32 MiB                          # hard ceiling #1 (landed cap)
rmcp_handler.rs:287 ContentBlock::image(b64, mime)           # b64 shipped as-is
main.rs SSE      16 MiB event cap, immovable (rmcp pub(crate)) # hard ceiling #2
```

End-to-end a capture costs ~2.7× its PNG size on the wire plus ~3 full
in-memory copies per hop. The error path does the same:
`crates/tdmcp-mcp/src/outcomes.rs:88 failed_one_with_image` attaches soft-fail
(black/uniform frame) images inline too.

## 2. Findings (ranked)

| # | Finding | Severity |
|---|---------|----------|
| **F1** | **`inspect include:["content"]` DAT text is unbounded.** `bridge/tdmcp_bridge/inspect.py:190 _dat_content` returns full `.text`; no cap exists bridge-side or Rust-side (the only content truncation test, `bridge/tests/test_inspect_summary.py:956`, covers shader *consumers*, not text). A multi-MB tableDAT overflows the IPC frame — the same session-killing class
the limits pass fixed for `execute_python`/`capture` (inspect slipped
through) — or at best megabytes dumped into agent context. | High (latent crash) |
| **F2** | Transport ceilings force quality down, not just safety: `CAPTURE_MAX_SIZE=1536px` (`constants.py:46`) exists *only* because of pipe/SSE caps. Full-res looks at large comps are impossible today by design. | High (capability ceiling) |
| **F3** | Every inline image lands in model context even when unwanted; non-vision clients get multi-MB dead-weight base64 blocks. | Medium |
| **F4** | Latency/memory: GIL-held `json.dumps` of MB-scale strings per capture ×3 buffer copies across hops. | Medium |
| **F5** | Tool description says "`top=native TOP JPEG`" (`tools.rs:158`) but code ships PNG (`capture.py:226`). Stale doc; also a hint that JPEG would be a 5–20× win on noisy TOPs. | Low |

## 3. Design decision — hybrid artifact spool (chosen)

Options weighed: file spool + reference (**chosen**) vs MCP resources (client
support inconsistent, still in-band) vs chunking (complexity, no gain) vs
raise-caps-again (dead end — SSE cap is fixed inside rmcp).

Two refinements over "everything goes to files":

1. **Hybrid threshold.** Inline ≤ `ARTIFACT_INLINE_MAX_BYTES` (256 KiB raw,
   pre-base64): single-call UX, federation-safe, zero regression, unchanged
   shapes. Spill only above the threshold. This is the design, not a stopgap.
2. **The bridge writes the file, not the wire.** TD Python has direct fs
   access: `saveByteArray` → `open(path,'wb').write(raw)` skips base64
   entirely; the JSON response shrinks to `{artifact:{id, path, bytes, mimeType}}`.
   Agents consume screenshots exactly as they already do elsewhere — with
   their own local file/image readers. Efficiency vs today: removes the +33% expansion, three full
   copies, all ceiling pressure; adds one sequential write + one local read.
   Strictly faster above ~100 KiB, identical below. Vision-token cost
   unchanged (same pixels).

Supporting facts this design relies on (verified 2026-08-26):

- `HandshakeResponse` (`crates/tdmcp-ipc/src/handshake.rs:40`) already carries
  optional daemon→TD fields with back-compat semantics (`idle_dead_secs`,
  `max_call_wait_secs`) — the spool dir rides this exact pattern.
- Bridge `.py` is embedded via `include_dir!`
  (`crates/tdmcp-daemon/src/install.rs:15`), extracted to `{data_dir}/bridge/`
  at startup (same-version refresh included), FS-loaded by TD after handshake.
  Deploy = kill daemons → rebuild → `tdmcp-daemon ensure` → restart TD
  processes. **No phase touches `bootstrap.py`/`tox_callbacks.py` → no tox
  repack anywhere.**
- Per-pid FIFO job queue serializes dispatch → no concurrency guard needed
  inside capture writes.
- rmcp promotion reads `mimeType` from the bridge payload
  (`crates/tdmcp-mcp/src/rmcp_handler.rs:273`) → JPEG rides existing plumbing.
- Daemon has `data_dir` (`crates/tdmcp-config/src/lib.rs:26` `APP_DIR_NAME =
  "tdmcp-rs"`; override `advanced.data_dir` `lib.rs:194`) and long-running
  timer loops (heartbeat/idle) a TTL sweeper can piggyback on.
- GUI does **not** consume captures today (`gui/src/lib.rs:151 load_rgba` is
  tray icons only).
- Federation proxies whole tool calls master→slave; no image-specific
  handling anywhere in `federation.rs`.

## 4. Phases

### Phase 0 — Bound `inspect` content (correctness fix)

**Files:** `bridge/tdmcp_bridge/constants.py`, `bridge/tdmcp_bridge/inspect.py`,
`diagnostics/catalog.yaml`, `crates/tdmcp-diagnostics/src/codes.rs`,
`bridge/fixtures/limits.json` (+ both parity tests), `docs/CONTRACT.md`.

1. New const `DAT_CONTENT_MAX_BYTES = 256 * 1024` in `constants.py`; mirror in
   `limits.json` + Rust parity test (`crates/tdmcp-daemon/tests/limits_parity.rs`).
2. In `inspect.py`:
   - `_dat_content`: when `_text_bytes(text)` exceeds the cap → truncate at a
     **UTF-8 char boundary** under the budget, set `truncated: true`, keep true
     total `bytes`, attach house-style
     `truncation{field:"content.text", limit, code:"tdmcp.op.content_truncated",
     message, mitigation:["read slices via execute_python","export the DAT and read the file"]}`.
   - `_shader_stage_from_ref` stage texts and `_shader_content` `compileResult`:
     same cap + marker. `compileState` classification runs on the full string
     before truncation.
3. Register `tdmcp.op.content_truncated` in `codes.rs` + `catalog.yaml`
   (severity info).
4. Rust side stays passthrough (envelope forwards unknown keys); update any
   verbatim cap text in `tools.rs` inspect description if it mentions size.

**Tests:** pytest in `bridge/tests/test_inspect_summary.py`: over-cap shape;
exactly-at-limit untouched; multibyte cut at char boundary decodes cleanly;
shader stage + compileResult variants. **Live check:** create a >256 KiB
tableDAT in TD, `inspect` it → truncated shape returned, bridge session alive.

### Phase 1 — Artifact spool for capture (local file delivery)

**Files:** `crates/tdmcp-ipc/src/handshake.rs`, daemon handshake construction +
spool sweeper (`crates/tdmcp-daemon/src/{main.rs,bridge.rs}`),
`bridge/tdmcp_bridge/{state.py,transport.py,capture.py,constants.py}`,
`crates/tdmcp-mcp/src/{tools.rs,schema.rs,rmcp_handler.rs,outcomes.rs}`,
`crates/tdmcp-mcp/tests/fixtures/schemas/capture.json`,
`bridge/tests/test_capture_max_size.py`, docs.

1. **Spool channel:** `HandshakeResponse` gains
   `#[serde(default)] artifact_spool_dir: Option<String>`; daemon sends
   `{data_dir}/artifacts`; Python stashes it in `state.py`. Absent (old
   daemon) → bridge always delivers inline (graceful both-way back-compat).
2. **New arg** `deliver: "auto"|"inline"|"file"` (default `auto`) on capture;
   schema + fixture + description updated together.
3. **Bridge behavior** in `_capture_top_image`:
   - `inline` → today's shape exactly.
   - `auto`: ≤ threshold inline; above → write raw PNG straight to
     `{spool}/{pid}/{unix_ms}-{uuid4}.png` (**no base64 ever computed**) and
     return `{ok:true, …, delivered:"file",
     artifact:{id, path(abs), bytes, mimeType}}` — `imageBase64` omitted.
   - `file` with no spool dir → curated error `tdmcp.artifact.no_spool`
     (mitigation: use inline/auto). Write failure → `tdmcp.artifact.write_failed`;
     `auto` falls back inline if payload is SSE-safe (<8 MiB) else the error.
   - **Native-res unlock (fixes F2):** `CAPTURE_MAX_SIZE=1536` now applies to
     inline only. When resolved size exceeds 1536 and a spool dir exists →
     force file delivery with ceiling `CAPTURE_FILE_MAX_SIZE = 8192` (reject
     beyond, mirroring `SCRIPT_MAX_BYTES` pre-flight style).
4. **Daemon sweeper:** periodic task deleting spool files older than 24 h +
   sweep on startup. Fixed defaults, no new config surface.
5. **rmcp_handler:** `delivered:"file"` results stay structured-only plus one
   short text block ("wrote N-byte PNG to <path> — open it with your
   image/file reader"); no image block. Soft-fail error images stay inline
   this phase (tiny by construction: black/uniform frames).

**Tests:** pytest threshold split, forced-file native res, no-spool error,
write-failure fallback; Rust handler leaves file results un-promoted while
small captures still promote as today. **Live check:** >256 KiB TOP → artifact
exists on disk, opens via agent image reader, MCP response contains no base64;
small capture still returns an image block; native-res (>1536px) capture
succeeds via file where it previously rejected.

### Phase 2 — Serve artifacts over HTTP + federation-safe fallback

**Files:** route alongside existing admin routes
(`crates/tdmcp-daemon/src/admin.rs`) or a sibling module,
`bridge/tdmcp_bridge/capture.py`, federation proxy touch-point.

1. `GET /artifacts/{pid}/{id}.{ext}` on the daemon's axum router: strict parse
   (`pid` = `[0-9]+`, `id` = `[0-9a-f]{32}`, `ext` ∈ {png,jpg}) → zero
   traversal surface; 404s use the curated JSON envelope like other admin
   routes; Content-Type from ext whitelist. Same loopback/trust model as
   existing admin routes — document, don't invent auth.
2. **Federation rule:** on remote pids `deliver:auto` resolves to **inline
   only** (slave payloads never spill to a path meaningless on the master
   machine). Explicit slave-side `deliver:"file"` → response carries
   `artifact.url` built from the slave's known address instead of a bare path.
   If the master/slave address book makes URL construction unreliable, ship
   the inline-only rule and defer `url` behind a flag.
3. GUI dashboard artifact browsing: **out of scope**.

**Tests:** valid fetch 200 + correct mime; malformed id → curated 404;
traversal attempts rejected. **Live check:** curl a real artifact id from a
running daemon; federated-pid capture still returns a usable result.

### Phase 3 — `format` param + `execute_python` result spill

**Files:** `bridge/tdmcp_bridge/{capture.py,execute.py,constants.py}`,
`crates/tdmcp-mcp/src/tools.rs` (CaptureParams + descriptions),
`crates/tdmcp-mcp/tests/fixtures/schemas/capture.json`, docs.

1. `format: "png"|"jpg"` (default `png`) on image capture paths:
   `saveByteArray(".jpg")`, `mimeType:"image/jpeg"`. Lossless stays default;
   fixes F5 honestly. Black/uniform classification unaffected
   (`numpyArray`-based, runs pre-encode).
2. `execute_python`: on `RESULT_MAX_BYTES` overflow, spill the **full** result
   to `{spool}/{pid}/<id>.txt` and return truncated preview +
   `artifact:{id,path,bytes,mimeType:"text/plain"}` beside the existing
   truncation metadata (keeps the never-discard-work contract, adds
   full-fidelity recovery).
3. Verbatim description strings updated in `tools.rs` (mechanical lockstep).

**Tests:** jpg mime/extension plumbing + result-spill shape; Rust promotion
accepts `image/jpeg` (already generic — assert). **Live check:** noisy TOP
captured both formats → jpg materially smaller; 5 MB script result returns
preview + readable `.txt` artifact.

## 5. Per-phase gate ("curated review audit/test pass")

Every phase ends with, in order, before the next starts:

1. `cargo clippy --workspace --all-targets -- -D warnings`
2. `cargo test --workspace` and `python -m pytest bridge/tests`
3. **Parity sweep** (shared values move in one change): `constants.py` ↔
   `tools.rs` consts *and description strings* ↔ `bridge/fixtures/limits.json`
   ↔ `CONTRACT.md` rows.
4. **Live MCP pass** (repo hard rule: MCP-first, never PASS from code alone):
   `taskkill /IM tdmcp-daemon.exe /F` → `cargo build --workspace` →
   `tdmcp-daemon ensure` → restart TD instance(s) → drive the phase's live
   checks through the real MCP surface; record observed outputs as evidence.
5. **Self-audit round** over the phase diff: latent bugs (encoding boundaries,
   old/new daemon↔bridge mixes, error-path regressions), machinery that
   doesn't pay rent, doc drift; fix before moving on.

Each phase lands as its own commit; working-tree checkpoint between phases.

## 6. Non-goals

No MCP resources surface, no chunked transfer, no GUI artifact browser, no
config keys beyond the phases above, no compression/zstd layer. Small payloads
stay inline forever.
