# TODO — Enforce typing across protocols

> Durable policy + backlog. Prefer principles over file inventories — code
> moves; these decisions should not. Update when a **decision** changes, not
> when a path renames.

Constitution already wants newtypes at boundaries (`Pid`, `OpPath`) and
catalog-backed diagnostics. This doc locks how we enforce types across the
four JSON surfaces: **MCP tools**, **bridge IPC**, **admin HTTP**, **diagnostics**.

---

## Verdict

| Surface | Target |
| --- | --- |
| **MCP tool args** | One Rust type → deserialize **and** `inputSchema` (`schemars` / rmcp `Parameters<T>`). No hand-maintained parallel schema. |
| **Bridge IPC** | Typed envelope + per-method params/results. Method is an enum, not a free string. |
| **Admin HTTP** | Share the same fleet/status types as the GUI client — no untyped JSON bags. |
| **Diagnostics** | Keep typed envelope + YAML catalog; CI: every emitted `tdmcp.*` ⊆ catalog. |

**Stack (locked):** Rust serde + `schemars` as SSOT. Golden JSON fixtures gate
drift. MCP `*Params` and bridge wire structs are **different shapes** (daemon
strips `pid`, maps methods) — do not unify them into one type. TD bridge stays
dependency-light: TypedDict / stubs + shared fixtures — **not** pydantic or
codegen into TD unless we explicitly accept a vendored dep.

`serde_json::Value` / `dict` is OK only for **dynamic TD trees** (e.g. inspect
payload insides). Request envelopes and known result shells stay typed.

**Do not:** binary IDL on the JSON pipe (Protobuf/Cap'n); JSON Schema files as
primary SSOT; fixtures-only without derived MCP schemas.

---

## Important notes

1. **Four wires, one discipline** — MCP, bridge IPC, admin, diagnostics all
   speak JSON but must not grow independent ad-hoc shapes. New fields land in
   typed Rust first, then fixtures, then Python.
2. **MCP schema ≡ deserialize** — if `list_tools` can advertise it, serde must
   accept it (and reject what the schema forbids). Hand-authored schema match
   arms are a bug magnet.
3. **Version layers stay separate** — MCP protocol date ≠ daemon semver ≠
   bridge `protocolVersion`. Breaking bridge wire bumps bridge protocol and
   rejects mismatch at handshake; breaking tool schema bumps daemon semver +
   goldens.
4. **Queue labels ≠ wire methods** — task-queue display names may differ from
   IPC `method`; both should hang off one enum (or an explicit map), never two
   free strings.
5. **Unknown fields** — prefer forbid on agent-facing MCP/bridge inputs;
   loosen only with a protocol bump.
6. **Response envelope** — JSON fallback and rmcp Streamable HTTP must not
   silently diverge forever; unify or document as two intentional surfaces.
7. **Catalog is law** — soft-failure codes from Rust or Python must exist in
   `diagnostics/catalog.yaml` in the same change.

---

## Backlog (coarse)

- [ ] Enum-ify stringly MCP/bridge fields; introduce `Pid` / `OpPath` newtypes
- [ ] Derive MCP schemas from param types; delete parallel hand schemas
- [ ] Enforce bridge protocol / min-daemon at handshake
- [ ] Typed `BridgeMethod` + per-method params/results (Rust + thin Python types)
- [ ] Golden fixtures + CI drift gate; catalog completeness check
- [ ] Typed admin/GUI fleet responses; typed IPC error object
- [ ] Typed MCP success shells + optional `outputSchema`; align response wrappers

---

## Still open

- Shared types crate vs keep payloads next to mcp/ipc
- Queue PascalCase labels: keep as display-only, or rename to wire method
- Unify MCP JSON-fallback wrapper with rmcp structured content?
