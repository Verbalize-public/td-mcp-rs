# Public Release TODO — td-mcp-rs

Curated gap report for a first proper public release. Repo state audited:
`main` @ `7113ff4`, version `0.1.1`, GitHub repo **private**
(`Verbalize-public/td-mcp-rs`), no tags, no releases.

Related: [`CURATED_REVIEW.md`](CURATED_REVIEW.md) (code audit, waves A–C
fixed), [`DELIVERY.md`](DELIVERY.md) (packaging internals),
[`../TODOLIST.md`](../TODOLIST.md) (feature backlog — *not* release scope).

## Verdict

The core is release-grade: curated-review waves A–C are closed, the quality
gate (`scripts/check.ps1` / `check.sh`: fmt + clippy `-D warnings` + cargo
test + bridge Python tests) is established, and the live-TD E2E gate is
largely proven on Windows. What is missing is everything *around* the code:
**no installer story, no CI, no LICENSE file, no user guide, no IDE
integration docs, and zero macOS validation**. No architectural blockers.

---

## 1. Distribution — easy install for a non-technical user (biggest gap)

Current state: build from source with `cargo build --release` (README
"Install"). Unacceptable for a layman audience.

| # | Item | Notes |
| --- | --- | --- |
| 1.1 | **Prebuilt binaries + one-line installer** | Adopt `cargo-dist` (or equivalent release workflow): tag push → Windows `.exe`/`.zip` + macOS artifacts attached to a GitHub Release, with `curl … \| sh` and `irm … \| iex` one-liners. Portable zip is enough — the tray daemon needs no installer. |
| 1.2 | **Tagging + CHANGELOG** | No git tags exist. Cut `v0.2.0`, add `CHANGELOG.md` (repo has no changelog at all), keep semver from there. |
| 1.3 | **macOS validation** | README claims "Compatible with MAC OS and windows", but every E2E run record in [`E2E_CHECKLIST.md`](E2E_CHECKLIST.md) is Windows (TD 099, Windows). Unix socket path exists in bridge transport, but it is unproven. Need: full E2E checklist run on macOS, tray/toast behavior check (RISKS R3 is Windows-only), and a codesigning/notarization decision (unsigned tray apps hit Gatekeeper friction). |
| 1.4 | **First-run UX check** | `ensure` / auto-extract of bridge + catalog + bootstrap `.tox` is already good (`DELIVERY.md` § Assets). Verify on a clean machine that Cursor auto-spawn → tray → drop `.tox` → first `fleet` call works with zero manual steps beyond the docs. |
| 1.5 | Optional / later | winget package, Homebrew tap, crates.io publish. Not blockers; binary + one-liner is the v1 path. |

## 2. Documentation — user guide + IDE coverage

| # | Item | Notes |
| --- | --- | --- |
| 2.1 | **README rewrite** | Typos ("wou", "mac os"); **tool table is stale** — lists 6 tools but `ToolName` (`crates/tdmcp-mcp/src/tools.rs:54`) has 8 (`mutate_nodes`, `api_help` missing); headline claims WIP features (master/slave orchestration, dialog auto-approval, open/close TD windows, offline tox injection) — move these to a clearly-labeled Roadmap section or cut them; no screenshot/GIF of the tray dashboard; no troubleshooting section. |
| 2.2 | **IDE integration guide** | Only Cursor is documented (`mcp.tdmcp.example.json`). Add copy-paste configs for: Claude Desktop, Claude Code, VS Code (Copilot / Cline), Windsurf, and generic Streamable HTTP clients (power users already have `http://127.0.0.1:9860/mcp/rpc`). One page, one snippet per client, both stdio and HTTP variants. |
| 2.3 | **User guide** | A `docs/USER_GUIDE.md`: install → drop `bootstrap.tox` → tray dashboard tour → first tool call → FAQ. Must cover: `keep_alive` / ~30s auto-exit, port conflicts, the Cursor parallel-call client quirk (E2E_CHECKLIST § Client quirks), same-version asset refresh (`install --force`). |
| 2.4 | **Doc drift** | `README.md:134` and `AGENTS.md:23` link to `TODO_ENFORCE_TYPE.md` — file does not exist. Fix or remove both pointers (already flagged in CURATED_REVIEW, still open). |
| 2.5 | **Docs triage for public exposure** | Decide what ships: `CONTRACT.md`, `CONFIG.md`, `TESTING.md`, `E2E_CHECKLIST.md`, `DELIVERY.md` are good public engineering docs; `CONSTITUTION.md`, `CURATED_REVIEW.md`, `GUI_WIREFRAME.md`, `docs/audit/` are internal — keep (they read well) but link them from a docs index rather than the README table. |
| 2.6 | **Examples** | One minimal demo `.toe` or `.tox` + a short "prompts that work" gallery (inspect → mutate → capture loop). Strongest possible onboarding artifact for an MCP server. |

## 3. Codebase — open defects and hygiene

From [`CURATED_REVIEW.md`](CURATED_REVIEW.md) (post-waves, still open):

| # | Item | Severity |
| --- | --- | --- |
| 3.1 | M5 — Windows restart pipe handoff race (TD can briefly attach to draining daemon; `first_pipe_instance` spent on failed create) | Medium |
| 3.2 | M6 — stdio annotate is "latest matching name"; two concurrent stdio clients cross-label `/admin/mcp-sessions` | Medium |
| 3.3 | M7 — dual MCP response shaping (`server.rs` vs `rmcp_handler.rs`) hand-synced; drift risk | Medium |
| 3.4 | Optional: codegen `codes.rs` from `catalog.yaml` (tests already bidirectional) | Low |

E2E rows never checked live ([`E2E_CHECKLIST.md`](E2E_CHECKLIST.md)):
5b (idle disconnect), 8c (`includeLogs:false`), 9b (failure logs context),
12b (`uniform_frame`), 20/21 (editor_context live), M21 (flat error shape
over axum + rmcp), M22 (mid-frame pipe timeout). These are the difference
between "tested on my machine" and "verified contract".

Repo hygiene:

- `fleet_req.json` tracked at root — dev leftover; delete or gitignore.
- `.cursor/plans/api_help_mcp_tool_14aa7762.plan.md` tracked, deletion
  currently uncommitted — commit; decide whether `.cursor/` belongs in a
  public repo at all.
- `.benchmarks/`, `.pytest_cache/` present on disk (pytest ignored;
  `.benchmarks` is not in `.gitignore`) — ignore or remove.
- Root `TODOLIST.md` is a raw notes file (typos, no structure) — either
  clean it into a real backlog or fold into a Roadmap doc before public eyes.

## 4. CI/CD and repo infrastructure

| # | Item | Notes |
| --- | --- | --- |
| 4.1 | **No `.github/` at all** | Add GitHub Actions CI mirroring `scripts/check.*`: `cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace`, bridge `python -m unittest`, on `windows-latest` **and** `macos-latest`. |
| 4.2 | **Release workflow** | Tag → build matrix → GitHub Release with checksums (cargo-dist provides this out of the box). |
| 4.3 | Community files | `CONTRIBUTING.md` (thin wrapper around `CONSTITUTION.md`), issue templates, `SECURITY.md` — the daemon binds localhost HTTP; a one-paragraph threat model ("localhost only, no auth — do not expose the port") belongs in docs. |
| 4.4 | Repo flip | Currently **private** with empty description. Set description/topics, then make public only after LICENSE (§5) and README rewrite (§2.1) land. |

## 5. Licensing

| # | Item | Notes |
| --- | --- | --- |
| 5.1 | **No LICENSE file** | `Cargo.toml` declares `license = "MIT"` but there is no `LICENSE` at root. Hard blocker for public. Decide copyright holder (org vs personal). |
| 5.2 | Dependency audit | All deps look permissive (MIT/Apache-2.0/ISC) — verify with `cargo deny` and commit the config; the `egui`/`eframe` stack and `rmcp` licenses specifically. |

## 6. Positioning

README opens with "This is not another cheap MCP for touchdesigner" — for a
public release, back it: a short comparison table (multi-window, multi-MCP-
consumer, resilient IPC + task queue honesty, resurrection, context-aware
panes) and the demo GIF (§2.6) do more than the claim. Park WIP headline
features (§2.1) on a Roadmap.

---

## Suggested release waves

| Wave | Contents | Exit criteria |
| --- | --- | --- |
| **R1 — blockers** | LICENSE file, README rewrite (tool table, roadmap split, GIF), doc-drift fixes (`TODO_ENFORCE_TYPE` pointers, `fleet_req.json`, `.cursor/`), CHANGELOG + tag `v0.2.0`, GitHub Actions CI (win + mac), repo made public | Public repo, green CI on both OS, clean `git status`, README quickstart verified start-to-finish |
| **R2 — install** | cargo-dist release pipeline, Windows + macOS artifacts with one-line installers, full macOS E2E run record, clean-machine install walkthrough | Non-technical user installs and reaches first `fleet` call with ≤ 1 pasted command + dropping the `.tox` |
| **R3 — polish** | M5/M6/M7, remaining E2E rows (5b, 8c, 9b, 12b, 20/21, M21, M22), USER_GUIDE + IDE guide expansion, SECURITY/CONTRIBUTING/templates, examples gallery | Zero known Medium+ defects, E2E checklist fully checked on both platforms |

**Deferred (post-v0.2, from `TODOLIST.md`):** per-project component
blacklist, sequenced tool calls, comment/annotation awareness, single-
instance pid default, palette/component gallery awareness, master/slave
orchestration, dialog auto-approval, offline tox injection.

## Top 5, if nothing else

1. LICENSE file + make repo public (legal blocker, 10 minutes)
2. Prebuilt binaries + one-line installer via cargo-dist (the "layman" gap)
3. GitHub Actions CI on Windows + macOS (trust signal + catches §3)
4. README rewrite + IDE guide beyond Cursor (first impression)
5. One full macOS E2E run (backs the cross-platform claim or kills it)
