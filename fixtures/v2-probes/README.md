# V2 probe fixtures

Evidence captured by `scripts/probes/v2/*.ps1` against TouchDesigner 2025.32460
(2026-08-25) while the offline project I/O contract was being designed and
verified. Files under `r0/` are load-bearing beyond documentation:

- `sample_text_envelope_60b.bin` is parsed by a test in
  `crates/tdmcp-projectio/src/sidecar.rs` (the `.text` sidecar envelope
  grammar is pinned to this real sample).
- `scripts/probes/v2/*.ps1` re-run against these fixtures.
- Gate G-L4 in `docs/LINUX_SUPPORT.md` re-opens the probe project under Wine.

| File | What it proves |
| --- | --- |
| `r1_live.toe` | Live-session copy-save used as expansion input (contains tox-sourced `tdmcp_rs` + `e2e_kit` subtrees — dragged tox subtrees expand to ordinary per-op grammar, no opaque payload needed) |
| `r1c_long.toe.toc` / `r1b_env.toe.toc` | Real `.toc` shape: LF-only, no BOM, tree-walk order, extension-carrying paths |
| `sample_text_envelope_60b.bin` | `.text` sidecar v2 envelope: `"2\n"` + u32LE(42) + 4×u32LE(1) + tag `0x02` + u32BE(60) + UTF-8 body (header = exactly 27 bytes) |
| `sample_text_envelope_empty.bin` | Same envelope with length 0 (empty DAT body) |
| `r3_authored.toe` (+`.toc`) | Packed result of a hand-authored 6-line `.n` + one `.toc` entry after strict-LF rewrite — loaded successfully by real TD (TD re-derives canonical toc order itself) |
| `flagship*.toe` + `flagship_final.toe.<ts>.bak` | Live probe project iterated during the probes; the `.bak` is a real TD save-collision artifact (TD writes `.N.toe` siblings + `Backup/` dirs on name clashes) |
