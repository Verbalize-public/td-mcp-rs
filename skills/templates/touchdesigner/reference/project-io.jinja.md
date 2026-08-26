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
| `project_install_bridge` | Rewrite the three bridge DAT bodies inside a project (backup + verify) |

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
callbacks — all three are rewritten together. `strategy=ensure` skips when
payloads already match.

## When offline vs live

Prefer `project_unpack` + surgical edits for bulk/structural changes; prefer
live operate for anything that needs cook feedback. After packing, verify with
`spawn_td` + `execute_python` before shipping.
