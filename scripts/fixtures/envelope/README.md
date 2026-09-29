# Sanitized public version-2 envelope fixtures

These are **synthetic** cases, hand-written to pin the envelope the version 2
output contract puts around a response. They are not sampled from any recorded
session.

They cover the distinctions that break *silently*:

- `captured` and `unavailable` are different states, and an unavailable field
  carries a reason. A caller that reads one as the other — or as an absent
  field — cannot tell from the text.
- `missing` and `unavailable` are counted separately in coverage: a path that
  does not exist is not the same evidence as a path that exists and could not
  be read.
- A capability is `Unknown` until its backend is actually built. No capability
  is claimed from a declaration.
- A `compact` request that could not be compact says so, with a reason, and
  does not report compact support for that request.

**Coverage note.** `ObservedValue` has four states — `unrequested`, `missing`,
`unavailable`, `captured` — but this producer only ever emits two of them
(`captured` and `unavailable`); `targetClock` is unconditionally unavailable
because no tool requests a target clock, and no field is marked `unrequested`.
The four-state vocabulary itself is pinned by the `tdmcp-core` unit tests. These
fixtures cover what the shipped producer actually produces, and do not claim the
states it cannot reach.

Each file is a **case**, not a recorded response: a query, a legacy value, the
requested options, and the expectations.
`crates/tdmcp-mcp/tests/envelope_fixtures.rs` builds each envelope through the
real `record_observation` + `wrap_v2` and checks the expectations against it.
Storing a whole expected envelope would freeze hashes, timestamps and the
per-process instance id, which change every run and would assert nothing.

| Fixture | What it pins |
| --- | --- |
| `captured-and-unavailable-stay-distinct` | a captured window is not an absent one, and an unavailable one says why |
| `no-capability-is-claimed-from-a-declaration` | an unbuilt backend is reported Unknown, never Supported |
| `compact-fallback-is-reported-not-passed-off` | a compact request that could not compact says so, and claims no compact support |
| `compact-served-is-decoded-back-exactly` | a compact document reconstructs the legacy value byte-for-byte |
| `coverage-accounting-does-not-inflate` | requested, returned, missing and unavailable are counted separately |
| `no-view-request-is-never-compacted` | omitting the view is a full response, and is not recorded as a compact one |
