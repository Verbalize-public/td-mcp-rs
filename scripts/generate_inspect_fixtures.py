"""Generate the sanitized public inspect fixtures.

The replay corpus recorded from real sessions is private: it carries session
ids, project paths, parameter values and authored comments. W0's contract is
that no private corpus is published, so the checked-in fixtures are built from
synthetic payloads that were written for this purpose.

The fixtures are deliberately adversarial rather than representative. They are
shaped to reach the branches the codec actually has to get right, and every one
of them was hand-written, not copied or resampled from a recorded session.

Usage:
    python scripts/generate_inspect_fixtures.py
    python scripts/generate_inspect_fixtures.py --check
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "scripts" / "fixtures" / "inspect"


def param(name: str, value=None, mode: str = "CONSTANT") -> dict:
    """One parameter row, in the shape inspect actually emits."""
    if value is None:
        return {"name": name}
    return {"evaluation": {"available": True}, "mode": mode, "name": name, "val": value}


def node(path: str, op_type: str, params: list, **extra) -> dict:
    row = {
        "childCount": 0,
        "children": [],
        "comment": "",
        "errors": [],
        "family": op_type[:3].upper(),
        "ok": True,
        "opType": op_type,
        "params": params,
        "path": path,
        "timing": {"available": True},
        "warnings": [],
    }
    row.update(extra)
    return row


def payload(call_seq: int, response: dict) -> dict:
    """Wrap a response in the same envelope the corpus uses."""
    return {
        "provenance": {
            "session": "synthetic",
            "callSeq": call_seq,
            "resultSeq": call_seq + 1,
            "source": "synthetic",
            "args": {"pid": 1, "paths": [], "include": ["params"]},
        },
        "response": response,
    }


def build() -> list[tuple[str, str, dict]]:
    """Return (name, expected_plan, payload) triples.

    `expected_plan` is enforced by `inspect_corpus_replay.rs` against what the
    codec actually decides, so a fixture cannot quietly stop reaching the path
    it exists to cover. It is one of `compact`, `full:no_transforms`,
    `full:not_smaller`, `full:traversal_budget`, `full:serialization_budget`,
    or `full:transform_budget`.
    """
    cases: list[tuple[str, str, dict]] = []

    # A uniform CONSTANT default across every row: the case a names-vector
    # transform can actually compress. Every row shares `mode`, so factoring it
    # out loses nothing.
    #
    # This one carries enough rows to amortize the compact envelope. A
    # three-parameter payload does not: the planner measured it at 469 bytes
    # full and declined with `not_smaller`, which is the correct decision but
    # leaves the names-vector path untested. A fixture that is documented as
    # reaching a transform has to actually reach it.
    cases.append((
        "uniform-constant-default",
        "compact",
        payload(1, {"nodes": [node(
            f"/project1/fx/gain{index}",
            "constantDAT",
            [param(f"param{param_index}", param_index / 4) for param_index in range(40)],
        ) for index in range(3)], "ok": True, "timing": {"available": True}}),
    ))

    # Mixed modes in one array. A names-vector encoder must NOT factor a
    # default here: the EXPRESSION rows would be misreported as CONSTANT.
    cases.append((
        "mixed-modes-no-default",
        "full:not_smaller",
        payload(2, {"nodes": [node(
            "/project1/fx/mixed",
            "expressionDAT",
            [param("value", 1.0, "CONSTANT"), param("driven", "frame", "EXPRESSION")],
        )], "ok": True, "timing": {"available": True}}),
    ))

    # Positional nulls and explicit false/zero. These must survive the round
    # trip unchanged: `null` is not the same as absent, `false` is not the
    # same as zero, and `0` is not the same as "no value".
    cases.append((
        "falsy-and-null-values",
        "full:no_transforms",
        payload(3, {"nodes": [node(
            "/project1/fx/falsy",
            "constantDAT",
            [
                param("nullval", None, "CONSTANT"),
                param("falseflag", False, "CONSTANT"),
                param("zeroval", 0, "CONSTANT"),
                param("emptystr", "", "CONSTANT"),
            ],
        )], "ok": True, "timing": {"available": True}}),
    ))

    # Signed zero and a float that only round-trips through enough precision.
    # -0.0 must not collapse to 0.0 on the way through the codec.
    cases.append((
        "signed-zero-and-precision",
        "full:not_smaller",
        payload(4, {"nodes": [node(
            "/project1/fx/numeric",
            "constantDAT",
            [
                param("negzero", -0.0, "CONSTANT"),
                param("tiny", 1.2345678901234567e-17, "CONSTANT"),
                param("big", 1.7976931348623157e308, "CONSTANT"),
            ],
        )], "ok": True, "timing": {"available": True}}),
    ))

    # A wide integer that does not survive an IEEE-754 double. A JS client
    # using JSON.parse loses precision here, which is exactly why the fixture
    # exists and why the Rust test must compare text, not numbers.
    cases.append((
        "wide-integer-precision",
        "full:not_smaller",
        payload(5, {"nodes": [node(
            "/project1/fx/ints",
            "constantDAT",
            [
                param("big_i64", 9007199254740993, "CONSTANT"),
                param("negative_big", -9007199254740993, "CONSTANT"),
                param("u64_max", 18446744073709551615, "CONSTANT"),
            ],
        )], "ok": True, "timing": {"available": True}}),
    ))

    # A per-node failure. The row is present and reports its own error, so a
    # lossy encoder must not drop it or flatten it into a status at the top.
    cases.append((
        "node-with-error",
        "full:no_transforms",
        payload(6, {"nodes": [{
            "childCount": 0,
            "children": [],
            "errors": [{"code": "op.missing", "message": "node is not in the project"}],
            "family": "MIS",
            "ok": False,
            "opType": "missing",
            "params": [],
            "path": "/project1/fx/ghost",
            "warnings": [],
        }], "ok": False, "timing": {"available": True}}),
    ))

    # Unavailable observations. `{"available": false}` is different from an
    # absent key and different from an empty list; all three must survive.
    cases.append((
        "unavailable-observations",
        "full:not_smaller",
        payload(7, {"nodes": [{
            "childCount": 0,
            "children": [],
            "errors": [],
            "family": "DAT",
            "ok": True,
            "observations": {
                "errors": {"available": False},
                "params": {"available": False},
                "warnings": {"available": True},
            },
            "opType": "executeDAT",
            "params": [param("a", 1, "CONSTANT"), param("b", 2, "CONSTANT")],
            "path": "/project1/fx/unavailable",
            "warnings": [],
        }], "ok": True, "timing": {"available": True}}),
    ))

    # Identical api_help rosters across rows, which is the roster transform's
    # precondition: same members in the same order means the repeated list can
    # be named once without loss.
    roster = ["save", "load", "reset"]
    cases.append((
        "identical-member-roster",
        "compact",
        payload(8, {"nodes": [node(
            f"/project1/fx/roster{index}",
            "panelDAT",
            [param("index", index, "CONSTANT") for index in range(3)],
            apiHelp=roster,
        ) for index in range(3)], "ok": True, "timing": {"available": True}}),
    ))

    # A paged child list. This is a captured prefix, not the whole subtree,
    # and the continuation must not be presented as if it were complete.
    cases.append((
        "paged-children",
        "full:no_transforms",
        payload(9, {"nodes": [{
            "childCount": 1200,
            "children": [{"path": f"/project1/fx/paged/child{index}"} for index in range(3)],
            "childrenPage": {"cursor": "opaque-cursor", "returned": 3, "total": 1200},
            "errors": [],
            "family": "COM",
            "ok": True,
            "opType": "folderCOMP",
            "params": [],
            "path": "/project1/fx/paged",
            "warnings": [],
        }], "ok": True, "timing": {"available": True}}),
    ))

    # Deep nesting, to reach the traversal budget rather than the transform
    # path. The codec admits values up to `max_depth = 64` levels, and this
    # payload nests 80 wrapper objects beneath the response root, so the planner
    # must decline. Whatever it decides, the value must round-trip intact.
    deep = {"depth": 80, "value": "leaf"}
    for _ in range(80):
        deep = {"child": deep}
    cases.append((
        "deep-nesting",
        "full:traversal_budget",
        payload(10, {"nodes": [node(
            "/project1/fx/deep",
            "constantDAT",
            [param("payload", deep, "CONSTANT")],
        )], "ok": True, "timing": {"available": True}}),
    ))

    # Strings that LOOK like control-plane data. A codec that scans for
    # markers instead of taking structure must not reinterpret these.
    cases.append((
        "control-shaped-literals",
        "full:not_smaller",
        payload(11, {"nodes": [node(
            "/project1/fx/literals",
            "constantDAT",
            [
                param("traps", {
                    "codec": {"name": "tdmcp.output_views", "version": 1},
                    "transforms": [{"kind": "names_vector", "path": ["nodes", 0, "params"]}],
                }, "CONSTANT"),
                param("control_text", "\u0000\u0007 not a control sequence", "CONSTANT"),
            ],
        )], "ok": True, "timing": {"available": True}}),
    ))

    # A single node with no parameters at all: nothing to transform, so the
    # encoder must decline and return the full value untouched.
    cases.append((
        "nothing-to-transform",
        "full:no_transforms",
        payload(12, {"nodes": [node("/project1/fx/bare", "nullCOMP", [])],
                     "ok": True, "timing": {"available": True}}),
    ))

    # Store the required plan inside each fixture, so the Rust test asserts
    # against the fixture's own data rather than a table that lives in the test.
    for _name, plan, document in cases:
        document["provenance"]["expectPlan"] = plan
    return cases


def serialize(cases: list[tuple[str, str, dict]]) -> str:
    """Render the fixture set, one file per case, deterministically ordered."""
    parts = []
    for name, _expected, document in cases:
        text = json.dumps(document, indent=2, sort_keys=True, ensure_ascii=False)
        parts.append(f"{name}.json:\n{text}\n")
    return "\n".join(parts)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify the checked-in fixtures match this generator, changing nothing",
    )
    args = parser.parse_args()

    cases = build()
    if args.check:
        stale = []
        for name, _expected, document in cases:
            path = OUT_DIR / f"{name}.json"
            if not path.is_file():
                stale.append(f"missing: {path.relative_to(ROOT)}")
                continue
            on_disk = json.loads(path.read_text(encoding="utf-8"))
            if on_disk != document:
                stale.append(f"differs: {path.relative_to(ROOT)}")
        readme = OUT_DIR / "README.md"
        expected_readme = render_readme(cases)
        if not readme.is_file() or readme.read_text(encoding="utf-8") != expected_readme:
            stale.append(f"differs: {readme.relative_to(ROOT)} (table is generated)")
        if stale:
            print("public inspect fixtures are out of date:", file=sys.stderr)
            for line in stale:
                print(f"  {line}", file=sys.stderr)
            return 1
        print(f"public inspect fixtures match the generator ({len(cases)} cases)")
        return 0

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    expected = {f"{name}.json" for name, _, _ in cases}
    for stale in OUT_DIR.glob("*.json"):
        if stale.name not in expected:
            stale.unlink()
    for name, _expected, document in cases:
        text = json.dumps(document, indent=2, sort_keys=True, ensure_ascii=False)
        (OUT_DIR / f"{name}.json").write_text(text + "\n", encoding="utf-8")
    (OUT_DIR / "README.md").write_text(render_readme(cases), encoding="utf-8")
    print(f"wrote {len(cases)} synthetic inspect fixtures to {OUT_DIR}")
    return 0


def render_readme(cases: list[tuple[str, str, dict]]) -> str:
    """Render the README, with the table generated from the same data the
    fixtures are built from.

    The table used to be hand-written, and it claimed a fixture reached the
    names-vector path when the planner had actually measured it as
    `not_smaller`. Generating it from `expected_plan` means a stale claim is
    fixed by re-running this script rather than by someone trusting a comment.
    """
    rows = "\n".join(
        f"| `{name}` | `{plan}` | {PURPOSES[name]} |" for name, plan, _ in cases
    )
    return README.replace("{{TABLE}}", rows)


# What each fixture is FOR, in the words of the contract. Paired with the
# expected plan above so a reader can see both the intent and the decision the
# codec actually makes.
PURPOSES = {
    "uniform-constant-default": "a uniform CONSTANT default is factored out losslessly",
    "mixed-modes-no-default": "a non-uniform default is refused, not misreported",
    "falsy-and-null-values": "`null` / `false` / `0` / `\"\"` stay distinct from absent",
    "signed-zero-and-precision": "`-0.0` and full float precision survive",
    "wide-integer-precision": "integers past 2^53 keep their exact digits",
    "node-with-error": "a per-node failure is not flattened or dropped",
    "unavailable-observations": "`available:false` differs from absent and from empty",
    "identical-member-roster": "an identical member roster is named once",
    "paged-children": "a captured prefix is not presented as complete",
    "deep-nesting": "nesting past the depth limit declines rather than truncating",
    "control-shaped-literals": "literal codec-looking data is never interpreted",
    "nothing-to-transform": "nothing to transform means the full value is returned",
}


README = """# Sanitized public inspect fixtures

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
{{TABLE}}

## What these fixtures do not establish

They do not measure cost. The byte figures in the ledger come from the private
corpus replay, and are **serialized bytes, not tokens** — no tokenizer is
available on the build host, so no token claim is made anywhere.
"""


if __name__ == "__main__":
    raise SystemExit(main())
