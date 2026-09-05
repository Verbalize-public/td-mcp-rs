# Operating skills

Author in `templates/` and register cards in `MANIFEST.yaml`.
The daemon serves one source in two forms: MCP resources at
`tdmcp://docs/<id>`, and Markdown files for filesystem-based skill clients.
`claude-skills/` is the checked-in filesystem render used by the Claude plugin.

## Write useful cards

- Describe **when to use the card** in its manifest description.
- Keep the umbrella short: route to focused reference cards instead of
  duplicating them. Every card must be reachable from `operate`.
- Use progressive detail. Simple edits should not require a planning ceremony;
  complex or destructive changes need explicit scope and verification.
- Distinguish a failed check from missing evidence. Do not claim live success
  from code, but do not label untested platforms as proven defects.
- Keep project-specific paths, handoff notes, and future plans out of shipped cards.

Each card uses a matching H1, a short orientation, its instructions,
`## Related`, and a `**Canonical:**` source line. Cross-reference through Jinja:

```jinja
{{ skill("python-api") }}
{{ skill_read("opsketch-notation") }}
```

These render to MCP resource instructions or relative Markdown links.
Do not hardcode either form in a template.

## Validate and render

Test the changed instructions against a representative task. Live behavior
changes need live TD evidence; editorial/routing changes need rendering,
reference checks, and a review of the resulting card. State limitations honestly.

```sh
cargo test -p tdmcp-mcp template_pack
cargo run -p tdmcp-daemon -- skills render --dest claude-skills
git add claude-skills
cargo test -p tdmcp-daemon claude_plugin_skills_match_rendered_output
```

Render in the same change as any template or manifest edit. Plugin installation
does not regenerate files. Do not silence the drift or reachability tests.

Export elsewhere with `tdmcp-daemon skills render --dest ./td-skills`.
The entry point is `touchdesigner/SKILL.md`.
