# V2 probe fixtures (V2-0)

Evidence captured by `scripts/probes/v2/*.ps1` against TouchDesigner 2025.32460
(2026-08-25). Findings and their consequences live in
[`docs/archive/SKILLS_CONTRACT_PROPOSAL.md`](../../docs/archive/SKILLS_CONTRACT_PROPOSAL.md) §6.1 and
[`docs/archive/V2_IMPLEMENTATION_PLAN.md`](../../docs/archive/V2_IMPLEMENTATION_PLAN.md).

| File | What it proves |
| --- | --- |
| `r1_live.toe` | Live-session copy-save used as expansion input (contains tox-sourced `tdmcp_rs` + `e2e_kit` subtrees) |
| `r1c_long.toe.toc` / `r1b_env.toe.toc` | Real `.toc` shape: LF-only, tree-walk order, extension-carrying paths |
| `sample_text_envelope_60b.bin` | `.text` sidecar v2 envelope: `"2\n"` + u32LE(42) + 4×u32LE(1) + tag `0x02` + u32BE(60) + UTF-8 body |
| `sample_text_envelope_empty.bin` | Same envelope with length 0 (empty DAT body) |
| `r3_authored.toe` (+`.toc`) | Packed result of hand-authored COMP entry (`project1/authored_v2`) after strict-LF toc fix — loaded successfully by real TD |
