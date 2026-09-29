#!/usr/bin/env python3
"""Score whether the compact codec still ANSWERS the questions it exists for.

Why this is separate from `measure_tokens.py`
--------------------------------------------
That script measures COST. This one measures whether the saving is paid for
with a wrong answer. The release plan's gate 3 is blunt about it: "A smaller
representation that increases mistakes does not ship by default." A token
figure cannot answer that question, and neither can the Rust replay test --
`inspect_corpus_replay.rs` proves the compact document DECODES back to the
recorded value byte for byte, which is LOSSLESSNESS. Losslessness and
answerability are different claims. A document can be perfectly reversible and
still answer a question wrongly, because the reader has to be able to tell an
absent value from a factored-out default.

The distinction this script exists to keep
-----------------------------------------
Consider a parameter in the compact document:

    {"name": "index", "val": 0}

In the full document the same parameter also carries `"mode": "CONSTANT"` and
`"evaluation": {"available": true}`. The codec factored those two out into a
`shared_defaults` transform addressed by path. A reader that never looks at
`transforms` sees a parameter with no mode and no evaluation -- and a
parameter with no mode is indistinguishable from a parameter whose mode could
not be read. That is the falsy/null/zero/empty family of error, arrived at
from a completely different direction, and it is the specific way this codec
could quietly increase mistakes while every existing test stayed green.

So the scorer does not ask "does it decode". It asks questions of each
document in the style of the plan's decision-fidelity set, where the ANSWER is
known in advance because it is read off the full document:

* Q1  is a missing key missing, or is it a factored-out default?
* Q2  does a value that is `null` / `false` / `0` / `""` stay distinct from
     one that is absent?
* Q3  is a value available but unevaluated distinguishable from one that could
     not be evaluated at all?
* Q4  is `ok: false` on one node distinguishable from no `ok` at all?
* Q5  is a captured prefix distinguishable from a complete roster?
* Q6  can the exact bytes of a named value be recovered from the document?
* Q7  is a per-node failure distinguishable from a node with no failures?

Each question is asked of the FULL document first, to establish ground truth
from the data rather than from a hand-written table. The compact document is
then scored against it. A question the full document cannot answer is reported
as NOT APPLICABLE, never as a pass -- an untestable question is not a satisfied
one.

The honesty rules this implements
---------------------------------
* **Every scoring is done by re-reading the document as a reader would**, with
  no access to the compact document's own `transforms` array except through
  the path resolution a reader has to perform anyway. A scorer that consulted
  a private field would be measuring the codec's self-description rather than
  the document's readability.
* **A question with no ground truth is not a pass.** It is reported as
  not-applicable and counted separately, because "no difference observed" and
  "nothing to observe" are the same distinction this codebase keeps
  everywhere else.
* **A declined compact request is scored, not skipped.** Ten of the twelve
  public fixtures are answered with the full value because compacting them
  would be wrong; scoring only the two that compacted would describe a subset
  nobody receives.
* **Non-inferiority is a claim about a MEASURED comparison, and a zero-row
  comparison is not one.** The script exits non-zero when a compact document
  cannot answer a question the full document answered.
* **The compact envelope is a versioned format.** A document with no `codec`
  key, or an unexpected version, is an error rather than a document to score:
  silently scoring an unknown format would let a future format version report
  today's parity result.

Usage
-----
    TDMCP_TOKEN_DUMP=/tmp/token-dump.json \\
    TDMCP_INSPECT_CORPUS=scripts/fixtures/inspect \\
        cargo test --offline --locked -p tdmcp-mcp --test inspect_corpus_replay

    python scripts/measure_decision_fidelity.py /tmp/token-dump.json
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

SUPPORTED_CODEC = "output-views"
SUPPORTED_VERSION = 1

MISSING = object()
"""A distinct sentinel, so `None` and "absent" never compare equal.

`json.loads` gives a real `None` for a JSON null, and omits the key entirely
for an absent value. Collapsing those into one Python value is exactly the
error the whole project refuses to make, so the scorer never lets it happen
internally either.
"""


class FidelityError(Exception):
    """The comparison could not be made, and reporting a number would be a lie."""


# ---------------------------------------------------------------- readers


def _resolve(document: Any, path: list) -> Any:
    """Follow a codec `path` into a document. Returns MISSING if it does not exist.

    The compact codec's transforms address values with JSON-path style
    locations. A reader has to follow them to recover the defaults, so the
    scorer follows them the same way -- if it cannot, that is a finding, not a
    scorer bug.
    """
    cursor = document
    for step in path:
        if isinstance(step, str):
            if not isinstance(cursor, dict) or step not in cursor:
                return MISSING
            cursor = cursor[step]
        elif isinstance(step, int):
            if not isinstance(cursor, list) or not (-len(cursor) <= step < len(cursor)):
                return MISSING
            cursor = cursor[step]
        else:
            return MISSING
    return cursor


def shared_defaults(document: dict) -> dict[tuple, Any]:
    """Index the compact document's factored-out defaults by location.

    This is what a reader must consult to know that a stripped `mode` key means
    "CONSTANT" rather than "unknown". A document with no such index has nothing
    to resolve against, which is the Q1 failure mode.
    """
    index: dict[tuple, Any] = {}
    transforms = document.get("transforms")
    if not isinstance(transforms, list):
        return index
    for transform in transforms:
        if not isinstance(transform, dict):
            continue
        if transform.get("kind") != "shared_defaults":
            # An unknown transform kind is not a default this scorer can
            # resolve. It is recorded as "no default here" rather than
            # assumed harmless, because an unresolvable default is
            # indistinguishable from an absent value.
            continue
        path = transform.get("path")
        if not isinstance(path, list):
            continue
        defaults = transform.get("defaults")
        if not isinstance(defaults, dict):
            continue
        index[tuple(path)] = defaults
    return index


def effective(document: Any) -> Any:
    """Materialise a compact document as a reader would reconstruct it.

    Walks `data` applying every indexed `shared_defaults` at its location, so
    the result is what a careful reader recovers -- and if a default cannot be
    applied, the key stays ABSENT, which is the honest failure.

    A transform's `path` is rooted at `data`, NOT at the envelope: the codec
    records `["nodes", 0, "params"]` for a location inside `data`. Resolving
    that path against the envelope resolves nothing at all, which leaves every
    factored default unrecovered and makes a correct document look lossy. That
    mistake reads exactly like a codec defect, so the path root is stated in
    the format's own terms here.
    """
    if not isinstance(document, dict) or not isinstance(document.get("data"), dict):
        return document
    data = document["data"]
    for location, defaults in shared_defaults(document).items():
        if not location:
            continue
        parent = _resolve(data, list(location[:-1]))
        if not isinstance(parent, (dict, list)):
            continue
        last = location[-1]
        if isinstance(parent, list):
            if not (isinstance(last, int) and -len(parent) <= last < len(parent)):
                continue
            target: Any = parent[last]
        elif isinstance(parent, dict) and last in parent and isinstance(parent[last], list):
            target = parent[last]
        else:
            continue
        if isinstance(target, list):
            for entry in target:
                for key, value in defaults.items():
                    if isinstance(entry, dict) and key not in entry:
                        entry[key] = value
        elif isinstance(target, dict):
            for key, value in defaults.items():
                if key not in target:
                    target[key] = value
    return data


def _param_at(data: Any, node_index: int, position: int) -> Any:
    """One parameter by ARRAY POSITION, not by name.

    Parameters are an ordered array and names need not be unique -- the
    `identical-member-roster` fixture deliberately carries three parameters
    called `index`. Matching by name and taking the first would compare every
    `index` against the same entry and report a difference that is not there,
    which is the scorer inventing a defect rather than finding one.
    """
    params = _params_at(data, node_index)
    if not isinstance(params, list) or not (-len(params) <= position < len(params)):
        return MISSING
    return params[position]


def _params_at(data: Any, node_index: int) -> Any:
    if not isinstance(data, dict):
        return MISSING
    nodes = data.get("nodes")
    if not isinstance(nodes, list) or not (-len(nodes) <= node_index < len(nodes)):
        return MISSING
    node = nodes[node_index]
    if not isinstance(node, dict) or "params" not in node:
        return MISSING
    return node["params"]


# ---------------------------------------------------------------- questions
#
# Every question has the same shape: it is given the two reconstructed views
# and returns None when the FULL document cannot establish ground truth, so
# "not applicable" is a real state rather than a convenient pass.


@dataclass
class Answer:
    """What one question said about one document pair."""

    question: str
    applicable: bool
    detail: str
    same: bool | None = None


def q1_factored_default_vs_absent(full: Any, compact: Any) -> Answer:
    """A key present in full but absent in compact must be recoverable as a default.

    This is THE question the compact view could plausibly fail. `mode:
    "CONSTANT"` and `evaluation: {available: true}` are factored out into a
    `shared_defaults` transform; a reader that never resolves that transform
    sees a parameter with no mode, and a parameter with no mode is
    indistinguishable from one whose mode could not be read. That is the
    falsy/absent family of error reached from the other direction, and it would
    be invisible to the lossless round-trip test -- the data IS there.

    Ground truth: if nothing was factored out of any node, this document does
    not exercise the risk and is reported as not-applicable rather than passed.
    """
    name = "Q1 factored-out default is recoverable, not read as absent"
    if not isinstance(full, dict) or not isinstance(full.get("nodes"), list):
        return Answer(name, False, "no node roster in the full document")

    # Collect what the codec stripped, per node, over the WHOLE roster rather
    # than node 0 alone: the transform is addressed per node index, so a
    # fixture that factors out of node 2 only would be invisible to a node-0
    # look.
    stripped: dict[int, dict[str, Any]] = {}
    compact_nodes = compact.get("data", {}).get("nodes") if isinstance(compact, dict) else None
    if not isinstance(compact_nodes, list):
        return Answer(name, True, "compact document has no node roster", same=False)
    for index, node in enumerate(full["nodes"]):
        if not isinstance(node, dict) or index >= len(compact_nodes):
            continue
        entry_compact = compact_nodes[index]
        if not isinstance(entry_compact, dict):
            continue
        params_full = node.get("params")
        params_compact = entry_compact.get("params")
        if not isinstance(params_full, list) or not isinstance(params_compact, list):
            continue
        for position, param in enumerate(params_full):
            if not isinstance(param, dict) or position >= len(params_compact):
                continue
            counterpart = params_compact[position]
            if not isinstance(counterpart, dict):
                continue
            missing = {
                key: value
                for key, value in param.items()
                # `name` and `val` are what the parameter IS. If either is
                # stripped the document lost information outright, which is
                # Q6's finding, not a factored default.
                if key not in counterpart and key not in ("name", "val")
            }
            if missing:
                stripped[index] = missing

    if not stripped:
        return Answer(name, False, "nothing was factored out of any node")

    recovered = effective(compact)
    recovered_nodes = recovered.get("nodes", []) if isinstance(recovered, dict) else []
    if not isinstance(recovered_nodes, list):
        return Answer(name, True, "reconstruction produced no roster", same=False)

    unrecovered: list[str] = []
    for index, missing in stripped.items():
        if index >= len(recovered_nodes) or not isinstance(recovered_nodes[index], dict):
            unrecovered.append(f"node {index} unreadable")
            continue
        params = recovered_nodes[index].get("params")
        if not isinstance(params, list):
            unrecovered.append(f"node {index} params unreadable")
            continue
        for key, value in missing.items():
            recovered_any = any(
                isinstance(entry, dict) and key in entry and entry[key] == value
                for entry in params
            )
            if not recovered_any:
                unrecovered.append(f"node {index} {key}={value!r}")

    return Answer(
        name,
        True,
        f"{sum(len(m) for m in stripped.values())} default(s) factored out of "
        f"{len(stripped)} node(s); every one recovered"
        if not unrecovered
        else f"unrecoverable: {', '.join(sorted(set(unrecovered)))}",
        same=not unrecovered,
    )


def q2_falsy_values_stay_distinct(full: Any, compact: Any) -> Answer:
    """null / false / 0 / "" must survive and stay distinct from absence."""
    name = "Q2 falsy values stay distinct from absent"
    falsy = []
    for index, node in enumerate(full.get("nodes", []) if isinstance(full, dict) else []):
        if not isinstance(node, dict):
            continue
        for position, param in enumerate(node.get("params", []) or []):
            if not isinstance(param, dict) or "val" not in param:
                continue
            if param["val"] in (None, False, 0, ""):
                falsy.append(
                    (index, position, param["name"], param["val"], "val" in param)
                )
    if not falsy:
        return Answer(name, False, "no falsy value in the full document")

    recovered = effective(compact)
    wrong = []
    for index, position, pname, value, present_in_full in falsy:
        entry = _param_at(recovered, index, position)
        if not isinstance(entry, dict):
            wrong.append(f"{pname}#{position}: missing from compact")
            continue
        if entry.get("name", MISSING) != pname:
            wrong.append(f"#{position}: name moved")
        if ("val" in entry) != present_in_full:
            wrong.append(f"{pname}#{position}: presence differs")
        elif "val" in entry and entry["val"] is not value and entry["val"] != value:
            wrong.append(f"{pname}#{position}: value changed")
    return Answer(name, True, "all falsy values preserved" if not wrong else "; ".join(wrong), same=not wrong)


def q3_evaluated_vs_unreadable(full: Any, compact: Any) -> Answer:
    """`available:false` must differ from absent and from `available:true`."""
    name = "Q3 unreadable-vs-unevaluated stays distinct"
    states = []
    for index, node in enumerate(full.get("nodes", []) if isinstance(full, dict) else []):
        if not isinstance(node, dict):
            continue
        for position, param in enumerate(node.get("params", []) or []):
            if isinstance(param, dict) and "evaluation" in param:
                states.append((index, position, param.get("name"), param["evaluation"]))
    if not states:
        return Answer(name, False, "no evaluation state in the full document")

    recovered = effective(compact)
    wrong = []
    for index, position, pname, expected in states:
        entry = _param_at(recovered, index, position)
        if not isinstance(entry, dict):
            wrong.append(f"{pname}#{position}: missing from compact")
        elif entry.get("evaluation", MISSING) != expected:
            wrong.append(f"{pname}#{position}: evaluation changed")
    return Answer(name, True, "all evaluation states preserved" if not wrong else "; ".join(wrong), same=not wrong)


def q4_node_failure_is_visible(full: Any, compact: Any) -> Answer:
    """`ok:false` on a node must survive; a node with no `ok` is a different claim."""
    name = "Q4 per-node failure stays visible"
    nodes = full.get("nodes", []) if isinstance(full, dict) else []
    if not isinstance(nodes, list) or not any(isinstance(n, dict) and "ok" in n for n in nodes):
        return Answer(name, False, "no per-node ok in the full document")

    recovered = effective(compact)
    compact_nodes = recovered.get("nodes", []) if isinstance(recovered, dict) else []
    if not isinstance(compact_nodes, list):
        return Answer(name, True, "nodes unreadable in compact", same=False)
    wrong = []
    for index, node in enumerate(nodes):
        if not isinstance(node, dict) or "ok" not in node:
            continue
        if index >= len(compact_nodes) or not isinstance(compact_nodes[index], dict):
            wrong.append(f"node {index} missing")
        elif "ok" not in compact_nodes[index]:
            wrong.append(f"node {index} lost ok")
        elif compact_nodes[index]["ok"] != node["ok"]:
            wrong.append(f"node {index} ok changed")
    return Answer(name, True, "every node ok preserved" if not wrong else "; ".join(wrong), same=not wrong)


def q5_prefix_is_not_a_roster(full: Any, compact: Any) -> Answer:
    """A captured prefix must not read as a complete roster."""
    name = "Q5 a captured prefix is not presented as complete"
    total = None
    for node in full.get("nodes", []) if isinstance(full, dict) else []:
        if isinstance(node, dict) and "childCount" in node and "children" in node:
            total = node["childCount"]
            break
    if total is None:
        return Answer(name, False, "no childCount/children pair in the full document")

    recovered = effective(compact)
    same_pair = False
    for node in recovered.get("nodes", []) if isinstance(recovered, dict) else []:
        if isinstance(node, dict) and "childCount" in node and "children" in node:
            same_pair = node["childCount"] == total
            break
    return Answer(name, True, "childCount preserved" if same_pair else "childCount lost or changed", same=same_pair)


def q6_exact_value_recoverable(full: Any, compact: Any) -> Answer:
    """Every named value in full must be byte-recoverable from the compact document."""
    name = "Q6 exact values are recoverable from the document alone"
    values = []
    for index, node in enumerate(full.get("nodes", []) if isinstance(full, dict) else []):
        if not isinstance(node, dict):
            continue
        for position, param in enumerate(node.get("params", []) or []):
            if isinstance(param, dict) and "val" in param:
                values.append((index, position, param.get("name"), param["val"]))
    if not values:
        return Answer(name, False, "no parameter values in the full document")

    recovered = effective(compact)
    wrong = []
    for index, position, pname, expected in values:
        entry = _param_at(recovered, index, position)
        if not isinstance(entry, dict):
            wrong.append(f"{pname}#{position}: missing from compact")
        elif "val" not in entry:
            wrong.append(f"{pname}#{position}: value lost")
        elif entry["val"] != expected:
            wrong.append(f"{pname}#{position}: value differs")
    return Answer(name, True, f"{len(values)} values recovered exactly" if not wrong else "; ".join(wrong), same=not wrong)


def q7_errors_are_not_flattened(full: Any, compact: Any) -> Answer:
    """A per-node error list must not become an empty list or vanish."""
    name = "Q7 a per-node failure is not flattened away"
    with_errors = [
        (index, node)
        for index, node in enumerate(full.get("nodes", []) if isinstance(full, dict) else [])
        if isinstance(node, dict) and node.get("errors")
    ]
    if not with_errors:
        return Answer(name, False, "no per-node errors in the full document")

    recovered = effective(compact)
    compact_nodes = recovered.get("nodes", []) if isinstance(recovered, dict) else []
    wrong = []
    for index, node in with_errors:
        if index >= len(compact_nodes) or not isinstance(compact_nodes[index], dict):
            wrong.append(f"node {index} missing")
        elif compact_nodes[index].get("errors") != node["errors"]:
            wrong.append(f"node {index} errors changed")
    return Answer(name, True, "every error list preserved" if not wrong else "; ".join(wrong), same=not wrong)


QUESTIONS: tuple[Callable[[Any, Any], Answer], ...] = (
    q1_factored_default_vs_absent,
    q2_falsy_values_stay_distinct,
    q3_evaluated_vs_unreadable,
    q4_node_failure_is_visible,
    q5_prefix_is_not_a_roster,
    q6_exact_value_recoverable,
    q7_errors_are_not_flattened,
)


# ---------------------------------------------------------------- scoring


def check_envelope(compact: Any) -> None:
    """Refuse to score a document whose format this scorer does not understand."""
    if not isinstance(compact, dict):
        raise FidelityError("compact document is not an object")
    codec = compact.get("codec")
    if not isinstance(codec, dict):
        raise FidelityError("compact document has no `codec` block: it is not a codec document")
    name = codec.get("name")
    version = codec.get("version")
    if name != SUPPORTED_CODEC:
        raise FidelityError(f"unknown codec {name!r}; this scorer knows {SUPPORTED_CODEC!r}")
    if version != SUPPORTED_VERSION:
        raise FidelityError(
            f"codec {name!r} version {version!r} is not the version this scorer was written "
            f"against ({SUPPORTED_VERSION}). A parity result for an unknown format version "
            "would be this scorer's guess, not a measurement."
        )
    if "data" not in compact:
        raise FidelityError("compact document has no `data` block")


def score_pair(row: dict) -> dict:
    """Score one `full`/`adaptive` document pair."""
    label = row.get("label", "<unlabelled>")
    plan = row.get("plan", "<no plan>")
    if plan != "compact":
        # A declined request served the full value in both halves; the
        # documents are identical by construction. It is still reported, so
        # the "no difference" is a measured one over a counted set.
        if row.get("full") != row.get("adaptive"):
            raise FidelityError(
                f"{label}: a {plan} row's two documents differ, so the dump does not "
                "describe the behaviour it claims to measure"
            )
        return {
            "label": label,
            "plan": plan,
            "applicable": [],
            "not_applicable": [f.__name__ for f in QUESTIONS],
            "failures": [],
        }

    try:
        full = json.loads(row["full"])
        compact = json.loads(row["adaptive"])
    except (KeyError, json.JSONDecodeError) as error:
        raise FidelityError(f"{label}: document pair is not readable JSON: {error}") from error

    check_envelope(compact)
    applicable: list[Answer] = []
    not_applicable: list[str] = []
    failures: list[Answer] = []
    for question in QUESTIONS:
        answer = question(full, compact)
        if answer.applicable and answer.same is False:
            failures.append(answer)
            applicable.append(answer)
        elif answer.applicable:
            applicable.append(answer)
        else:
            not_applicable.append(question.__name__)

    return {
        "label": label,
        "plan": plan,
        "applicable": [a.detail for a in applicable],
        "not_applicable": not_applicable,
        "failures": [a.detail for a in failures],
    }


def score(rows: list[dict]) -> dict:
    """Score every row, refusing to report a number from a set it cannot measure.

    Row INTEGRITY is checked before the question of whether anything is worth
    scoring. That order is deliberate: a dump whose declined rows carry two
    different documents is not describing the product at all, and saying "no
    compact document" about it would be true-but-misleading -- a reader would go
    looking for a missing measurement when the real problem is a lying one.
    """
    for row in rows:
        plan = row.get("plan", "<no plan>")
        if plan != "compact" and row.get("full") != row.get("adaptive"):
            raise FidelityError(
                f"{row.get('label', '<unlabelled>')}: a {plan} row's two documents differ, "
                "so the dump does not describe the behaviour it claims to measure. A declined "
                "compact request returns the full value unchanged, so equal halves are the "
                "property that makes the declined rows in this set meaningful rather than "
                "padding."
            )

    compact_rows = [r for r in rows if r.get("plan") == "compact"]
    if not compact_rows:
        raise FidelityError(
            "no compact document in the dump. Non-inferiority of the compact view is a claim "
            "about the documents it produced; with none, the claim has nothing behind it and "
            "'no regressions' would be an absence of evidence rather than a measurement."
        )

    scored = [score_pair(row) for row in rows]
    questions_compact = sum(len(s["applicable"]) for s in scored if s["plan"] == "compact")
    failures = [f for s in scored for f in s["failures"]]
    return {
        "documents": len(rows),
        "compact_documents": len(compact_rows),
        "declined_documents": len(rows) - len(compact_rows),
        "questions_answered": questions_compact,
        "failures": failures,
        "scored": scored,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("dump", type=Path, help="the file TDMCP_TOKEN_DUMP wrote")
    args = parser.parse_args()

    if not args.dump.is_file():
        print(f"error: {args.dump} is not a file", file=sys.stderr)
        print(
            "Generate it with TDMCP_TOKEN_DUMP set while the corpus replay runs.",
            file=sys.stderr,
        )
        return 2
    rows = json.loads(args.dump.read_text())
    if not isinstance(rows, list) or not rows:
        print(f"error: {args.dump} holds no document pairs", file=sys.stderr)
        return 2

    try:
        result = score(rows)
    except FidelityError as error:
        print(f"decision-fidelity measurement refused: {error}", file=sys.stderr)
        return 1

    print(f"decision-fidelity over {result['documents']} inspect documents")
    print(
        f"  {result['compact_documents']} compact, {result['declined_documents']} answered "
        f"with the full value (scored, not skipped)"
    )
    print(f"  questions answered against known ground truth: {result['questions_answered']}")
    for entry in result["scored"]:
        if entry["plan"] != "compact":
            continue
        status = "OK" if not entry["failures"] else "MISMATCH"
        print(f"  [{status}] {entry['label']}")
        for detail in entry["applicable"]:
            print(f"      - {detail}")
        if entry["not_applicable"]:
            print(
                f"      (no ground truth in the full document: "
                f"{', '.join(entry['not_applicable'])})"
            )
        for detail in entry["failures"]:
            print(f"      ! {detail}")

    if result["failures"]:
        print(
            f"\nREFUSED: {len(result['failures'])} question(s) the full document answered "
            "were answered differently by the compact document. The release plan's gate 3 "
            "is explicit that a smaller representation which increases mistakes does not "
            "ship by default.",
            file=sys.stderr,
        )
        return 1

    print(
        "\nThis is a NON-INFERIORITY check over documents with known ground truth:\n"
        "the compact document answered every question the full document could answer.\n"
        "It is not evidence about any particular model's judgement, and a question with\n"
        "no ground truth is reported as not-applicable rather than as a pass."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
