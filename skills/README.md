# Operating skills

Author in `templates/` and register cards in `MANIFEST.yaml`.
The daemon serves one source in two forms: MCP resources at
`tdmcp://docs/<id>`, and Markdown files for filesystem-based skill clients.
`claude-skills/` is the checked-in filesystem render used by the Claude plugin.

## Write useful cards

- Describe **when to use the card**, the next decision it supports, and important
  non-triggers in its manifest description. Related links are navigation, not a
  recursive reading requirement.
- Give each procedure a trigger, needed facts, next action, success evidence and
  failure/stop branch. Separate authority/protocol invariants from conditional
  task procedures and optional style. A scalar edit or read-only audit does not
  trigger the reusable-component release checklist.
- Own each rule in one canonical card; repeat only short safety cues with links.
  Preserve public IDs, file paths and important anchors when splitting references.
  A card cannot grant permissions, activate unavailable tools or override scope.
- Classify examples as complete MCP requests, Python execution scripts,
  owner-scoped expressions/callbacks, explanatory fragments or intentional
  negatives. Record context and expected outcome; schema validation alone cannot
  establish TD semantics. Negative examples must not look like recommended calls.
- Label evidence per claim: source-confirmed, unit-tested, native-observed on a
  named build, or unverified. Never apply a blanket live-verification badge to a
  reference based on one tested snippet. Future capabilities need explicit support
  checks and must not be taught as current APIs.
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

## Authoring registry and examples

`GUIDANCE.json` records the audience, trigger/non-trigger, canonical owner,
capability scope and migration disposition of every public ID. Its `ruleOwners`
map gives each behavioral rule one owner. This is validated authoring metadata,
not runtime permission enforcement or an intent executor. Keep manifest retrieval
descriptions and registry triggers identical.

`EXAMPLES.json` classifies every fenced template block, including explanatory
fragments and requests that need a `pid`/`steps` wrapper. Reviewed source hashes
make unclassified edits fail. Complete requests are checked against full derived
JSON Schema and the real Rust Serde decoders. Python fragments receive syntax
checks, not invented namespace or native acceptance. Intentional negative requests
must remain rejected. Native scripts list their exact context and evidence limits.
After an example changes, review its classification/context/outcome and update its
hash and arguments together; do not update fixtures just to hide a failing contract.

Install test dependencies from `scripts/requirements-test.txt`, then run
`python scripts/check_guidance.py`. This validator never executes project code
or fetches external schemas. Bridge semantic tests and scoped native observations
remain separate. Old resource IDs and important Python section anchors are kept;
detailed members live in `python-reference` and `parameter-reference`.

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
