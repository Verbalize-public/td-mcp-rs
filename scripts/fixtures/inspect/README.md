# Sanitized public inspect fixtures

These payloads are **synthetic**. They were hand-written for
`crates/tdmcp-mcp/tests/inspect_corpus_replay.rs` and are not derived from any
recorded session.

## Why they exist

The replay corpus measured against real sessions stays private: it carries
session ids, project paths, parameter values and authored comments. W0's
contract is that no private corpus is published, so the public tree gets
adversarial fixtures instead of a sample. Regenerate with:

    python scripts/generate_inspect_fixtures.py
    python scripts/generate_inspect_fixtures.py --check   # CI

## What each case is for

The second column is the plan the codec is *required* to choose, stored in each
fixture and asserted by `inspect_corpus_replay.rs`. It is not a description of
intent alone: if a fixture stops reaching the path it exists to cover, the test
fails instead of the documentation quietly going stale.

| Fixture | Required plan | Why it exists |
| --- | --- | --- |
| `uniform-constant-default` | `compact` | a uniform CONSTANT default is factored out losslessly |
| `mixed-modes-no-default` | `full:not_smaller` | a non-uniform default is refused, not misreported |
| `falsy-and-null-values` | `full:no_transforms` | `null` / `false` / `0` / `""` stay distinct from absent |
| `signed-zero-and-precision` | `full:not_smaller` | `-0.0` and full float precision survive |
| `wide-integer-precision` | `full:not_smaller` | integers past 2^53 keep their exact digits |
| `node-with-error` | `full:no_transforms` | a per-node failure is not flattened or dropped |
| `unavailable-observations` | `full:not_smaller` | `available:false` differs from absent and from empty |
| `identical-member-roster` | `compact` | an identical member roster is named once |
| `paged-children` | `full:no_transforms` | a captured prefix is not presented as complete |
| `deep-nesting` | `full:traversal_budget` | nesting past the depth limit declines rather than truncating |
| `control-shaped-literals` | `full:not_smaller` | literal codec-looking data is never interpreted |
| `nothing-to-transform` | `full:no_transforms` | nothing to transform means the full value is returned |

## What these fixtures do not establish

They do not measure cost. The byte figures in the ledger come from the private
corpus replay, and are **serialized bytes, not tokens** — no tokenizer is
available on the build host, so no token claim is made anywhere.
