# td-mcp-rs constitution

Law of the land for this Rust workspace. Inspired by sibling
[`touchdesigner-mcp-td/td-rs`](../touchdesigner-mcp-td/td-rs/docs/CONSTITUTION.md)
engineering law — **scoped down** for a daemon (no porq / gate-lock / roadmap
orchestration). Sources: [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/),
[Embark standard lints](https://github.com/EmbarkStudios/rust-ecosystem/issues/59),
[RFC 3389 workspace lints](https://rust-lang.github.io/rfcs/3389-manifest-lint.html).

Enforcement: root `Cargo.toml` `[workspace.lints]` inherited by every crate via
`[lints] workspace = true`. Local gate: `scripts/check.ps1` / `scripts/check.sh`.

## Resilience / never-panic

1. Prefer typed `Result` and catalog-backed diagnostics over crash on IPC,
   queue, MCP, and bridge paths.
2. Public APIs must not panic. Document **error** conditions (`Result` /
   diagnostics); do not document “panics if …” unless listed in
   [`RISKS.md`](RISKS.md).
3. Any choice that can crash or harm long-running stability must be
   **challenged**. Accept only when clearly justified. Record the exception in
   [`RISKS.md`](RISKS.md) in the **same change**.
4. **Stability outranks performance:** do not introduce `unwrap` / `expect` /
   `panic!` to “go faster.”

## Safety and style

- `unsafe_code` is **deny** at workspace level. Exactly one quarantined
  carve-out exists: `crates/tdmcp-ipc/src/winsec.rs` (Windows pipe security
  descriptor FFI; must expose safe functions only). Any further carve-out
  needs a new amendment + `RISKS.md` entry in the same change.
  (Amended 2026-08-23 from `forbid`: Win32 `CreateNamedPipe` security
  descriptors have no safe wrapper in std/tokio.)
- No `unwrap` / `expect` / `panic!` / `todo!` / `unimplemented!` in library
  code on release paths (exceptions only via `RISKS.md`).
- Prefer `?` and typed errors: **`thiserror` in libs**; **`anyhow` only in
  binaries** (`tdmcp-daemon`, `xtask`).
- `unused_must_use` is deny.

## Crate boundaries

| Crate | May know | Must not know |
| --- | --- | --- |
| `tdmcp-core` | domain types, `tdmcp-diagnostics` | `rmcp`, `axum`, IPC transports |
| `tdmcp-config` | TOML config file I/O + defaults | `rmcp`, axum, egui, IPC |
| `tdmcp-diagnostics` | catalog YAML, envelope types | MCP / IPC / axum |
| `tdmcp-ipc` | framing, named pipe / UDS | MCP tool schemas, egui |
| `tdmcp-mcp` | `rmcp`, core + diagnostics | egui, OS tray |
| `tdmcp-daemon` | composition root; optional `tdmcp-gui` under `gui` feature | business logic (thin wiring only) |
| `tdmcp-gui` | admin HTTP client, egui, `tdmcp-config` (lib consumed by daemon) | core queue internals, IPC wire |
| `tdmcp-test-support` | fake peer speaking real wire protocol | production binary paths |

## API guidelines

- Public types implement `Debug`; cheap types also `Clone` where appropriate.
- Error types implement `std::error::Error` + meaningful `Display`.
- Conversions via `From` / `TryFrom`.
- Struct fields private by default; expose accessors.
- Newtypes for process identity and operator paths at boundaries (not bare
  `String` / `u32` where a domain type exists).
- Document **error** conditions on public functions.
- Lib crates: `#![warn(missing_docs)]`.

## Factorization / DRY

Duplication is a smell to **investigate**, not an automatic extract. Wrong
abstraction is worse than duplication.

1. **Investigate before extract** — classify essential vs accidental duplication.
2. **Rule of three** — extract on the third stable use-site *or* when the block
   is a boundary bug magnet (paths, diagnostic construction).
3. **Smallest honest form** — local fn → enum/newtype → small generic → trait.
4. **Reject** single-impl traits “for later,” Util/Common bags, type theater.

## Clippy

Workspace warns on `clippy::all` and denies high-signal lints (`dbg_macro`,
`todo`, `unimplemented`, `unwrap_used`, `expect_used`, `panic`, `exit`, …).

Binaries may `#[allow(clippy::exit)]` only at the process boundary (`main`).
Tests may `#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]`
with a short reason.

## Diagnostics

1. Soft failures use typed codes from `diagnostics/catalog.yaml` (stable
   `tdmcp.*` strings) — no free-string-only error bags on the MCP surface.
2. New codes land with catalog entry + emitter + test in the same change.
3. Catalog completeness is enforced by a unit/integration test.

## Config

Precedence: **CLI args > env vars > RC file > built-in defaults**.

## Amendments

- Prefer fixing lint noise over blanket allows.
- Local `#[allow(...)]` must include a reason.
- New release-path panic / unwrap / expect / exit / unsafe allows land with a
  `RISKS.md` entry in the same change.
- Constitution changes land in the same PR as `[workspace.lints]` edits.

## Changelog

| Date | Note |
| --- | --- |
| 2026-07-29 | Gate 0 constitution established (scoped from td-rs law) |
| 2026-08-23 | Unsafe quarantine: `tdmcp-ipc::winsec` (RISKS R8); workspace `unsafe_code` forbid→deny |
