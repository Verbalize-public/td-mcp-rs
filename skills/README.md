# td-mcp-rs operate skills

Agent reference cards for TouchDesigner, authored as Jinja templates and
rendered into two output modes from one source.

## Source of truth

```text
skills/
├── MANIFEST.yaml        id → template/output path + title/description
├── templates/           Jinja templates (`.jinja.md`)
│   └── touchdesigner/
│       ├── SKILL.jinja.md
│       ├── reference/*.jinja.md
│       └── primer/*.jinja.md
└── README.md            (this file)
```

Every cross-reference uses a Jinja procedure, not a hardcoded URI:

```jinja
Depth: {{ skill("opsketch-notation") }}.
Before Python: {{ skill_read("python-api") }}.
```

## Two output modes

| Mode | `skill("python-api")` renders as | `skill_read("python-api")` renders as |
|------|----------------------------------|---------------------------------------|
| **MCP resources** | `` `tdmcp://docs/python-api` `` | `` `resources/read` `tdmcp://docs/python-api` `` |
| **Filesystem** | `` [`python-api`](./reference/python-api.md) `` | `` see [`python-api.md`](./reference/python-api.md) `` |

### MCP resources (harnesses that support `resources/read`)

Served by the daemon. The catalog is `resources/list`; each card is
`resources/read` `tdmcp://docs/<id>`. Cross-references are resource URIs.

### Filesystem export (any harness)

```text
tdmcp-daemon skills render --dest ./td-skills/
```

Renders every card with relative Markdown links (`./path/to/card.md`), so a
harness with only filesystem skills (DSH, Cursor skills dirs, etc.) reads them
without any `resources/read` or `tdmcp://` URI. The umbrella is
`touchdesigner/SKILL.md`; every referenced card is a sibling path.

## Catalog

The canonical id list lives in `MANIFEST.yaml`. MCP URIs are
`tdmcp://docs/<id>`. Filesystem paths are `touchdesigner/...` under the render
root (mirroring the `templates/` layout with `.jinja.md` → `.md`).

## Authoring a card (authoring contract)

Cards are verified operate knowledge, not README paraphrases. Before adding or
editing a card, hold these:

- **Retrieval first.** The MANIFEST `description` is the retrieval surface
  (`resources/list`): lead with WHAT + WHEN in the verbs an agent would type
  ("Use when inspecting…"), not a noun phrase.
- **Verify by running, last.** After editing, rebuild (`cargo build -p
  tdmcp-daemon`), render, and follow the rendered card line-by-line in a fresh
  session; any improvisation is a gap — fix the card. Facts you cannot verify
  live this session are marked BLOCKED/unverifiable, never shipped as truth.
- **No hardcoded links.** Every cross-reference uses `skill("id")` /
  `skill_read("id")` — never a literal `.md` path or a vague "mcp skill"
  phrase. The lint test
  (`cargo test -p tdmcp-mcp template_pack_cross_references_are_well_formed`)
  enforces this and that every id exists in the MANIFEST.
- **One skeleton.** Every card: `# <H1 matching the MANIFEST title>` → one-line
  orientation sentence naming the handle/tool → body → `## Related` → canonical
  line at the bottom. Non-trivial cards carry a Definition of Done checklist
  (see `reference/definition-of-done.jinja.md`).
- **Point, don't inline.** The umbrella routes; depth lives in `reference/`
  cards; primers stay condensed. Don't restate another card's content — link it.
- **No project-specific facts.** Shipped cards are generic operate knowledge.
  Project paths (`*.toe`, scratch zones) belong in project docs, not templates.
