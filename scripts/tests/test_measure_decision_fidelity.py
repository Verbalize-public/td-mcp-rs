"""The decision-fidelity check must refuse to report a result it did not earn.

`scripts/measure_decision_fidelity.py` exists because LOSSLESSNESS and
ANSWERABILITY are different claims. `inspect_corpus_replay.rs` proves the
compact document decodes back to the recorded value byte for byte; that says
the data is present, not that a reader can tell a factored-out default from an
absent value. The release plan's gate 3 turns on the difference: "a smaller
representation that increases mistakes does not ship by default."

The dangerous failure mode is a green report. A scorer that silently scores
nothing, or that reports a question it could not actually test, prints the same
"non-inferior" summary as one that tested everything -- and the first is
evidence of absence dressed as evidence of parity. So these tests are written
as refusals, and every one of them asserts the specific diagnostic, not merely
a non-zero exit: a script that refused for an unrelated reason would pass a
weaker test and mislead the next reader.

None of these tests needs a real corpus, a tokenizer, or TouchDesigner.
"""

import copy
import importlib.util
import json
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "measure_decision_fidelity",
    ROOT / "scripts/measure_decision_fidelity.py",
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot load the decision-fidelity script")
FIDELITY = importlib.util.module_from_spec(SPEC)
# `@dataclass` resolves annotations through `sys.modules[cls.__module__]`, so
# the module must be REGISTERED before it executes. Loading it without this
# line fails at class-definition time rather than at any use, which reads as a
# dataclass bug rather than a loader one.
sys.modules[SPEC.name] = FIDELITY
SPEC.loader.exec_module(FIDELITY)


def _node(path: str, params: list, **extra) -> dict:
    node = {
        "childCount": 0,
        "children": [],
        "comment": "",
        "errors": [],
        "family": "CON",
        "ok": True,
        "opType": "constantDAT",
        "params": params,
        "path": path,
        "timing": {"available": True},
        "warnings": [],
    }
    node.update(extra)
    return node


def _param(name: str, value=None, mode: str = "CONSTANT") -> dict:
    return {
        "evaluation": {"available": True},
        "mode": mode,
        "name": name,
        "val": value,
    }


def _full(nodes: list) -> str:
    return json.dumps({"nodes": nodes, "ok": True, "timing": {"available": True}})


def _compact(nodes: list, transforms: list) -> str:
    return json.dumps(
        {
            "codec": {"name": "output-views", "version": 1},
            "data": {"nodes": nodes, "ok": True, "timing": {"available": True}},
            "transforms": transforms,
        }
    )


def _row(full: str, adaptive: str, plan: str = "compact", label: str = "case") -> dict:
    return {"label": label, "plan": plan, "full": full, "adaptive": adaptive}


def _pair(params: list, stripped: list[str] = ("mode", "evaluation")) -> list:
    """A full/compact pair where `stripped` keys are factored into transforms."""
    full_params = [_param(f"p{i}", i) for i in range(len(params))]
    compact_params = [
        {k: v for k, v in param.items() if k not in stripped} for param in full_params
    ]
    defaults = {key: full_params[0][key] for key in stripped}
    transforms = [
        {
            "kind": "shared_defaults",
            "path": ["nodes", 0, "params"],
            "defaults": defaults,
        }
    ]
    return _row(
        _full([_node("/p/n0", full_params)]),
        _compact([_node("/p/n0", compact_params)], transforms),
    )


class ACorrectCompactDocumentScores(unittest.TestCase):
    def test_a_recoverable_default_is_reported_as_a_recovery(self):
        result = FIDELITY.score([_pair([0, 1, 2])])
        self.assertEqual([], result["failures"], "a correct document must not fail")
        self.assertEqual(1, result["questions_answered"] - 5)

    def test_every_stripped_key_is_counted_as_recovered(self):
        scored = FIDELITY.score_pair(_pair([0, 1, 2]))
        q1 = [d for d in scored["applicable"] if "default" in d]
        self.assertTrue(q1, "Q1 must apply when keys were factored out")
        self.assertIn("2 default(s) factored out", q1[0])
        self.assertIn("recovered", q1[0])

    def test_a_document_that_factors_out_nothing_is_not_applicable_not_passed(self):
        """The vacuity case: a compact document with no defaults to resolve.

        Q1's whole subject is a factored-out default. With none present there
        is nothing to test, and reporting that as a pass would let a set with
        no risk at all read as a clean bill of health.
        """
        full = _full([_node("/p/n0", [_param("p0", 1)])])
        compact = _compact([_node("/p/n0", [_param("p0", 1)])], [])
        scored = FIDELITY.score_pair(_row(full, compact))
        self.assertIn("q1_factored_default_vs_absent", scored["not_applicable"])
        self.assertEqual([], scored["failures"])


class TheScorerCanFail(unittest.TestCase):
    """Every mutation here is a document a reader would answer wrongly."""

    def _assert_refused(self, row, question_fragment):
        with self.assertRaises(FIDELITY.FidelityError) as caught:
            result = FIDELITY.score([row])
            # `score` does not raise; the exit code does. Model the main().
            raise AssertionError(
                f"expected a refusal, got {result['questions_answered']} answers"
            )

    def _failures_of(self, row):
        return FIDELITY.score([row])["failures"]

    def test_missing_transforms_block_makes_a_default_unreadable(self):
        """A reader with no `transforms` cannot tell CONSTANT from unknown."""
        row = _pair([0, 1, 2])
        document = json.loads(row["adaptive"])
        del document["transforms"]
        row["adaptive"] = json.dumps(document)
        self.assertTrue(self._failures_of(row), "a stripped reader must fail Q1")

    def test_a_default_aimed_at_the_wrong_node_is_a_wrong_answer(self):
        row = _pair([0, 1, 2])
        document = json.loads(row["adaptive"])
        document["transforms"][0]["path"] = ["nodes", 1, "params"]
        row["adaptive"] = json.dumps(document)
        self.assertTrue(self._failures_of(row), "a misaddressed default must fail Q1")

    def test_a_default_with_the_wrong_value_is_a_wrong_answer(self):
        row = _pair([0, 1, 2])
        document = json.loads(row["adaptive"])
        document["transforms"][0]["defaults"]["mode"] = "EXPRESSION"
        row["adaptive"] = json.dumps(document)
        self.assertTrue(self._failures_of(row), "a corrupted default must fail Q1")

    def test_dropping_a_nodes_parameters_fails(self):
        row = _pair([0, 1, 2])
        document = json.loads(row["adaptive"])
        document["data"]["nodes"][0].pop("params")
        row["adaptive"] = json.dumps(document)
        self.assertTrue(self._failures_of(row), "dropped parameters must fail")

    def test_a_node_ok_flipped_to_zero_fails(self):
        """`true` -> `0` is the falsy collapse this codebase exists to prevent."""
        row = _pair([0, 1, 2])
        document = json.loads(row["adaptive"])
        document["data"]["nodes"][0]["ok"] = 0
        row["adaptive"] = json.dumps(document)
        self.assertTrue(self._failures_of(row), "ok:0 must not pass for ok:true")

    def test_a_changed_parameter_value_fails(self):
        row = _pair([0, 1, 2])
        document = json.loads(row["adaptive"])
        document["data"]["nodes"][0]["params"][1]["val"] = 99
        row["adaptive"] = json.dumps(document)
        self.assertTrue(self._failures_of(row), "a changed value must fail Q6")

    def test_an_erased_error_list_fails(self):
        """An error that vanishes is a wrong answer even though nothing is malformed."""
        full = _full([_node("/p/n0", [_param("p0", 1)], errors=["boom"])])
        compact = _compact([_node("/p/n0", [_param("p0", 1)], errors=[])], [])
        self.assertTrue(self._failures_of(_row(full, compact)), "erased errors must fail Q7")


class ParameterOrderIsLoadBearing(unittest.TestCase):
    """Parameters are positional, and names need not be unique.

    The `identical-member-roster` fixture carries three parameters all called
    `index`. A scorer that matched by name and took the first would compare
    every `index` against the same entry and report a difference that is not
    there -- a scorer inventing a defect. It would also be unable to notice a
    genuine reordering, which is the check's actual purpose.
    """

    def _roster(self, compact_values: list) -> dict:
        full_params = [
            {"evaluation": {"available": True}, "mode": "CONSTANT", "name": "index", "val": v}
            for v in (0, 1, 2)
        ]
        compact_params = [{"name": "index", "val": v} for v in compact_values]
        defaults = {"evaluation": {"available": True}, "mode": "CONSTANT"}
        transforms = [
            {"kind": "shared_defaults", "path": ["nodes", 0, "params"], "defaults": defaults}
        ]
        return _row(
            _full([_node("/p/n0", full_params)]),
            _compact([_node("/p/n0", compact_params)], transforms),
        )

    def test_duplicate_names_in_order_pass(self):
        self.assertEqual([], FIDELITY.score([self._roster([0, 1, 2])])["failures"])

    def test_a_reordered_parameter_fails(self):
        """The same three values, out of order, is a different answer."""
        self.assertTrue(
            FIDELITY.score([self._roster([2, 1, 0])])["failures"],
            "a reordering of an ordered array must be caught",
        )

    def test_a_single_changed_value_among_duplicates_fails(self):
        self.assertTrue(
            FIDELITY.score([self._roster([0, 9, 2])])["failures"],
            "one changed value among duplicate names must be caught",
        )


class NullAndAbsentStayDistinct(unittest.TestCase):
    def _params(self):
        return [
            {"name": "nullval"},
            {"evaluation": {"available": True}, "mode": "CONSTANT", "name": "falseflag", "val": False},
            {"evaluation": {"available": True}, "mode": "CONSTANT", "name": "zeroval", "val": 0},
            {"evaluation": {"available": True}, "mode": "CONSTANT", "name": "emptystr", "val": ""},
        ]

    def test_a_faithful_null_preserves_the_distinctions(self):
        params = self._params()
        compact = [
            {k: v for k, v in p.items() if k not in ("mode", "evaluation")} for p in params
        ]
        defaults = {"evaluation": {"available": True}, "mode": "CONSTANT"}
        row = _row(
            _full([_node("/p/falsy", params)]),
            _compact(
                [_node("/p/falsy", compact)],
                [{"kind": "shared_defaults", "path": ["nodes", 0, "params"], "defaults": defaults}],
            ),
        )
        self.assertEqual([], FIDELITY.score([row])["failures"])

    def test_dropping_a_falsy_value_entirely_fails(self):
        """A `null` parameter and an absent parameter are different claims."""
        params = self._params()
        compact = [
            {k: v for k, v in p.items() if k not in ("mode", "evaluation")}
            for p in params
        ][1:]  # the null-valued parameter is gone
        row = _row(
            _full([_node("/p/falsy", params)]),
            _compact(
                [_node("/p/falsy", compact)],
                [
                    {
                        "kind": "shared_defaults",
                        "path": ["nodes", 0, "params"],
                        "defaults": {"evaluation": {"available": True}, "mode": "CONSTANT"},
                    }
                ],
            ),
        )
        self.assertTrue(FIDELITY.score([row])["failures"], "a dropped null must fail")


class TheScorerRefuses(unittest.TestCase):
    """A refusal with the wrong reason is still a refusal a reader would trust."""

    def _refuse(self, rows, fragment):
        with self.assertRaises(FIDELITY.FidelityError) as caught:
            FIDELITY.score(rows)
        self.assertIn(fragment, str(caught.exception))

    def test_no_compact_document_is_not_a_clean_result(self):
        """The core vacuity guard: nothing to compare is not parity."""
        rows = [_row("{}", "{}", plan="full")]
        self._refuse(rows, "no compact document")

    def test_a_declined_row_whose_halves_differ_is_refused(self):
        """The dump is claiming a behaviour it does not describe."""
        rows = [_row('{"a":1}', '{"a": 1}', plan="full", label="declined")]
        self._refuse(rows, "two documents differ")

    def test_an_unknown_codec_is_refused(self):
        row = _pair([0, 1, 2])
        document = json.loads(row["adaptive"])
        document["codec"]["name"] = "something-else"
        row["adaptive"] = json.dumps(document)
        self._refuse([row], "unknown codec")

    def test_an_unknown_codec_version_is_refused(self):
        """Scoring an unknown format version would report today's parity result.

        A future codec version could factor out a different set of keys. The
        questions below are written against version 1's transform kinds, so
        running them over version 2 would measure the wrong thing and call it
        a pass.
        """
        row = _pair([0, 1, 2])
        document = json.loads(row["adaptive"])
        document["codec"]["version"] = 2
        row["adaptive"] = json.dumps(document)
        self._refuse([row], "version")

    def test_a_missing_codec_block_is_refused(self):
        row = _pair([0, 1, 2])
        document = json.loads(row["adaptive"])
        del document["codec"]
        row["adaptive"] = json.dumps(document)
        self._refuse([row], "no `codec` block")

    def test_unreadable_json_is_refused(self):
        row = _pair([0, 1, 2])
        row["adaptive"] = "{not json"
        self._refuse([row], "not readable JSON")

    def test_an_unknown_transform_kind_is_not_silently_resolved(self):
        """A kind this scorer does not understand must not be treated as a default.

        Treating it as harmless is the dangerous branch: the key stays absent
        and the value reads as unknown, which is exactly the confusion the
        check exists to catch -- except now the check would be the cause.
        """
        params = [_param("p0", 1)]
        compact = [{"name": "p0", "val": 1}]
        row = _row(
            _full([_node("/p/n0", params)]),
            _compact(
                [_node("/p/n0", compact)],
                [
                    {
                        "kind": "some_future_factorisation",
                        "path": ["nodes", 0, "params"],
                        "defaults": {"mode": "CONSTANT", "evaluation": {"available": True}},
                    }
                ],
            ),
        )
        self.assertTrue(
            FIDELITY.score([row])["failures"],
            "an unresolvable transform must read as unrecoverable, not as fine",
        )


class DeclinedCallsAreCounted(unittest.TestCase):
    def test_a_declined_row_is_scored_and_counted_not_skipped(self):
        full = _full([_node("/p/n0", [_param("p0", 1)])])
        rows = [_pair([0, 1]), _row(full, full, plan="full", label="declined")]
        result = FIDELITY.score(rows)
        self.assertEqual(2, result["documents"])
        self.assertEqual(1, result["declined_documents"])
        # The declined row contributes no question because it has no compact
        # half to differ; it is present so the denominator is the whole set.
        self.assertEqual(1, result["compact_documents"])


if __name__ == "__main__":
    unittest.main()
