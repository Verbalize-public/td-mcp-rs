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
