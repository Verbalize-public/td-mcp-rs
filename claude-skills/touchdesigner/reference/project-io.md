# Project I/O — offline `.toe`/`.tox` work through official tools

Unpack, inspect-as-files, edit surgically, repack, and install the MCP bridge
into any TouchDesigner project — without a live TD connection.

## Tools

| Tool | Use |
| --- | --- |
| `td_installs` | Discover installs; `complete:false` = stub (no bin tools) |
| `project_unpack` | `.toe`/`.tox` → expand dir (`<name>.dir` + strict-LF `<name>.toc`) |
| `project_pack` | Expand dir → packed file (build-skew guarded) |
| `project_lint` | toc/filesystem consistency, duplicates, bridge DAT presence |
| `project_install_bridge` | Install the bridge into a project — rewrite it, or create it where absent (backup + verify) |

Every tool here is offline: no `pid`, no bridge, no running TD. They are exempt
from the sequential-bridged-call gate ([`tooling-concurrency`](./tooling-concurrency.md)).

## Reliability law (non-negotiable)

- Official-tool **exit codes lie in both directions** — success is judged by
  filesystem evidence only.
- `.toc` must be strict LF / no BOM. CRLF makes toecollapse emit a silent
  0-byte file.
- `.text` sidecars carry a 27-byte envelope; never hand-edit bytes around it —
  rewrite via the tools.
- Structural round-trip is guaranteed; byte-identical output is not.

## Bridge install

`project_install_bridge` rewrites `bootstrap`, `callbacks`, and
`tdmcp_exec` payloads from the daemon's embedded sources. The exec DAT mirrors
callbacks — all three are rewritten together.

| Arg | Default | Meaning |
| --- | --- | --- |
| `targetPath` | *(required)* | Packed `.toe`/`.tox` to modify |
| `strategy` | **`force`** | `force` always rewrites; `ensure` skips when payloads already match |
| `backup` | `true` | Writes `<name>.<ts>.bak` beside the target before replacing |

**No bridge yet?** The tool creates one. It expands the daemon's shipped
`bootstrap.tox` in staging, copies it under an unambiguous host COMP, and
appends the prefixed `.toc` lines (strict LF) — nothing is authored by hand.
Success returns `created:true`. When the project has no single obvious host
COMP, it refuses with `tdmcp.project.bridge_subtree_missing` rather than
guessing; name the host by unpacking first.

Success payload: `{ok, updated, created, rewritten, bytes}` — or
`{ok:true, updated:false, message}` when `strategy=ensure` found a match.

## Failure codes

All in the `tdmcp.project.*` family. The ones you will actually hit:

| Code | Means |
| --- | --- |
| `tool_missing` / `tool_pair_partial` | No usable install — run `td_installs` first |
| `source_not_found` / `not_packed_format` | Bad path, or the file is not a real `.toe`/`.tox` |
| `expand_failed` / `collapse_failed` | Official tool produced no valid artifact (exit code is *not* the judge) |
| `toc_invalid` / `toc_escape` | `.toc` unparseable, or an entry escapes the expand dir |
| `dest_exists` | Destination present and `overwrite` is `fail` (the default) — pass `overwrite:"replace"` to stash-and-restore |
| `build_skew` | Source `.build` ≠ selected install — pass `allowBuildSkew` only if you mean it |
| `bridge_subtree_missing` | No `tdmcp_rs` subtree *and* no unambiguous host COMP to create one in |

## When offline vs live

Prefer `project_unpack` + surgical edits for bulk/structural changes; prefer
live operate for anything that needs cook feedback. After packing, run
`project_lint` on the output first (offline, cheap), and only then `spawn_td`
+ `execute_python` for the live claim.

## Related

- Start / stop TD to verify a packed project: [`lifecycle`](./lifecycle.md)
- Compat dialogs after repacking across builds: [`popups`](./popups.md)
- What `.toe` / `.tox` are: [`primer/tox-toe-components`](../primer/tox-toe-components.md)
- Component packaging before you pack: [`component-checklist`](./component-checklist.md)

---

**Canonical:** [`project-io`](./project-io.md)