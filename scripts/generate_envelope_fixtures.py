"""Generate the sanitized public version-2 envelope fixtures.

The inspect fixtures (scripts/generate_inspect_fixtures.py) pin what the
COMPACT CODEC does to a payload. These pin the ENVELOPE around it: the four
observation states, the capability manifest, and the fallback reporting.

The unit of coverage is different on purpose. The inspect fixtures assert one
thing per file (which representation the planner chose); these assert the
distinctions the v1 envelope was designed to preserve, where getting one wrong
is silent. A caller that reads `unavailable` as `captured: null` cannot tell the
difference from the response text.

Each fixture is a CASE rather than a recorded response: a query, a legacy value,
the requested options, and the expectations. `envelope_fixtures.rs` builds the
envelope through the real `record_observation` + `wrap_v2` and checks the
expectations against it. Storing a whole expected envelope would freeze
hashes, timestamps and the per-process instance id, which change every run and
would make the fixture assert nothing.

Usage:
    python scripts/generate_envelope_fixtures.py
    python scripts/generate_envelope_fixtures.py --check
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "scripts" / "fixtures" / "envelope"


def legacy_read(node_count: int = 1, params_per_node: int = 1) -> dict:
    """A minimal, well-formed legacy read response.

    `params_per_node` exists because the codec factors defaults out of a single
    `params` array. One parameter per node leaves nothing to factor out, so a
    compact document costs more than it saves and the planner correctly declines
    with `not_smaller` — which is the right answer for the fallback case and the
    wrong shape for the case that has to reach the compact path.
    """
    return {
        "ok": True,
        "timing": {"available": False, "reason": "not requested"},
        "nodes": [
            {
                "childCount": 0,
                "children": [],
                "comment": "",
                "errors": [],
                "family": "CON",
                "ok": True,
                "opType": "constantDAT",
                "params": [
                    {
                        "evaluation": {"available": True},
                        "mode": "CONSTANT",
                        "name": f"value{param}",
                        "val": 1.0,
                    }
                    for param in range(params_per_node)
                ],
                "path": f"/project1/fx/const{index}",
                "warnings": [],
            }
            for index in range(node_count)
        ],
    }


# Each case: (name, purpose, query, legacy, options, expect).
#
# `expect` keys are an assertion vocabulary the Rust test understands. Every one
# of them names a distinction that is silent when broken, which is why they are
# spelled out rather than derived.
CASES: list[tuple[str, str, dict, dict, dict, dict]] = [
    (
        "captured-and-unavailable-stay-distinct",
        "a captured window is not an absent one, and an unavailable one says why",
        {"pid": 1, "paths": ["/project1/fx/const0"]},
        legacy_read(),
        {"contract": "v2"},
        {
            "contract": "v2",
            # This producer captures a monotonic interval and never requests a
            # target clock. Asserting the states it actually emits is the point:
            # an expectation copied from the type definition rather than from the
            # producer is an expectation that will rot.
            "window.monotonic.state": "captured",
            "window.targetClock.state": "unavailable",
            "presentation.requested": "full",
            "presentation.applied": "full",
            "outcome.effect": "none",
        },
    ),
    (
        "no-capability-is-claimed-from-a-declaration",
        "an unbuilt backend is reported Unknown, never Supported",
        {"pid": 1, "paths": ["/project1/fx/const0"]},
        legacy_read(),
        {"contract": "v2"},
        {
            "contract": "v2",
            # Every capability defaults to Unknown. Only the contract this
            # envelope actually used, and a compact view that was really
            # returned, may ever be anything else.
            "capabilities.outputV2.state": "supported",
            "capabilities.evidenceRead.state": "unknown",
            "capabilities.snapshotPaging.state": "unknown",
            "capabilities.exactMemberLookup.state": "unknown",
            "capabilities.guardedWrites.state": "unknown",
            "capabilities.numericalSampling.state": "unknown",
            "capabilities.operationStatus.state": "unknown",
        },
    ),
    (
        "compact-fallback-is-reported-not-passed-off",
        "a compact request that could not compact says so, and claims no compact support",
        {"pid": 1, "paths": ["/project1/fx/const0"]},
        # Too small for the compact envelope to pay for itself.
        legacy_read(node_count=1),
        {"contract": "v2", "view": "compact"},
        {
            "contract": "v2",
            "presentation.requested": "compact",
            "presentation.applied": "full",
            # A reason is required. A compact request answered with the full
            # value and no stated reason is the silent substitution the whole
            # presentation block exists to prevent.
            "presentation.fullReason": "present",
            "capabilities.compactView.state": "unknown",
        },
    ),
    (
        "compact-served-is-decoded-back-exactly",
        "a compact document reconstructs the legacy value byte-for-byte",
        {"pid": 1, "paths": ["/project1/fx/const0"]},
        # Measured, not guessed: one parameter per node cannot amortize the
        # compact envelope, so this has to be wide enough for a transform to
        # pay for itself. `a_served_compact_document_decodes_back_to_the_legacy_value`
        # fails if it ever stops reaching the compact path.
        legacy_read(node_count=3, params_per_node=40),
        {"contract": "v2", "view": "compact"},
        {
            "contract": "v2",
            "presentation.requested": "compact",
            "presentation.applied": "compact",
            "presentation.fullReason": "absent",
            "capabilities.compactView.state": "supported",
        },
    ),
    (
        "coverage-accounting-does-not-inflate",
        "requested, returned, missing and unavailable are counted separately",
        {
            "pid": 1,
            "paths": [
                "/project1/fx/const0",
                "/project1/fx/absent",
                "/project1/fx/unreadable",
            ],
        },
        {
            "ok": True,
            "nodes": [
                legacy_read()["nodes"][0],
                {
                    # The shape the bridge actually emits for a path that does
                    # not exist: `ok: false` WITH `code: tdmcp.op.not_found`
                    # (bridge/tdmcp_bridge/inspect.py:616-620). An earlier
                    # revision of this fixture left the code off, which routed
                    # the node to the `unavailable` branch instead — so the
                    # fixture named "accounting does not inflate" was
                    # exercising none of the inflation it claimed to, and a
                    # mutation that counted `missing` as `returned` survived
                    # against it. The code is what distinguishes the two.
                    "ok": False,
                    "path": "/project1/fx/absent",
                    "code": "tdmcp.op.not_found",
                    "message": "operator not found: /project1/fx/absent",
                },
                {
                    # A path that exists but could not be observed. Any failure
                    # code other than not_found lands here, and a node with no
                    # `ok` at all lands here too.
                    "ok": False,
                    "path": "/project1/fx/unreadable",
                    "code": "tdmcp.bridge.timeout",
                    "message": "timed out reading this node",
                },
            ],
        },
        {"contract": "v2"},
        {
            "contract": "v2",
            # One came back, one does not exist, one could not be read. All
            # three are named separately; a caller told "2 of 3 returned"
            # would be told a lie either way.
            "coverage.requested": 3,
            "coverage.returned": 1,
            "coverage.missing": 1,
            "coverage.unavailable": 1,
            "coverage.accountedFor": 3,
        },
    ),
    (
        "no-view-request-is-never-compacted",
        "omitting the view is a full response, and is not recorded as a compact one",
        {"pid": 1, "paths": ["/project1/fx/const0"]},
        # Deliberately compactable in shape. If the payload could not have
        # compacted anyway, this fixture would pass even if omission were
        # wrongly treated as a compact request.
        legacy_read(node_count=3, params_per_node=40),
        {"contract": "v2"},
        {
            "contract": "v2",
            "presentation.requested": "full",
            "presentation.applied": "full",
            "presentation.fullReason": "absent",
            "capabilities.compactView.state": "unknown",
        },
    ),
]


def build() -> list[tuple[str, dict]]:
    """Return (name, fixture) pairs in a stable order."""
    return [
        (
            name,
            {
                "provenance": {
                    # Synthetic by construction: nothing here is sampled from a
                    # recorded session, so there is no session id to leak.
                    "source": "synthetic",
                    "purpose": purpose,
                },
                "case": {
                    "query": query,
                    "legacy": legacy,
                    "options": options,
                    "expect": expect,
                },
            },
        )
        for name, purpose, query, legacy, options, expect in CASES
    ]


README = """# Sanitized public version-2 envelope fixtures

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
{{TABLE}}
"""


def render_readme() -> str:
    rows = "\n".join(
        f"| `{name}` | {json.loads(json.dumps(fixture))['provenance']['purpose']} |"
        for name, fixture in build()
    )
    return README.replace("{{TABLE}}", rows)


def write(check: bool) -> int:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    expected: dict[str, str] = {}
    for name, fixture in build():
        expected[f"{name}.json"] = json.dumps(fixture, indent=2, sort_keys=True) + "\n"
    expected["README.md"] = render_readme()

    if check:
        problems = []
        for filename, content in expected.items():
            path = OUT_DIR / filename
            if not path.exists():
                problems.append(f"missing: {path.relative_to(ROOT)}")
            elif path.read_text(encoding="utf-8") != content:
                problems.append(f"stale: {path.relative_to(ROOT)}")
        for path in sorted(OUT_DIR.iterdir()):
            if path.name not in expected:
                problems.append(f"unexpected: {path.relative_to(ROOT)}")
        if problems:
            for problem in problems:
                print(problem, file=sys.stderr)
            return 1
        print(f"envelope fixtures match ({len(expected) - 1} cases)")
        return 0

    for filename, content in expected.items():
        (OUT_DIR / filename).write_text(content, encoding="utf-8")
    print(f"wrote {len(expected) - 1} envelope fixtures to {OUT_DIR}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify the checked-in fixtures match this generator",
    )
    args = parser.parse_args()
    return write(args.check)


if __name__ == "__main__":
    raise SystemExit(main())
