"""The token measurement must refuse to report a number it did not earn.

`scripts/measure_tokens.py` exists because a token count is a different
measurement from a byte count, and converting one into the other with a
character ratio invents a figure. The dangerous failure mode is not a crash: it
is a plausible number produced by something that was not really a tokenizer, or
a number that quietly describes only the calls that were going to win anyway.

Every test here drives a path that would produce a misleading report if it did
not fail. That is why they are written as refusals rather than as arithmetic
checks, and why none of them needs the real tiktoken to be installed.
"""

import contextlib
import importlib.util
import io
import json
import sys
import tempfile
import types
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "measure_tokens", ROOT / "scripts/measure_tokens.py"
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot load the token measurement script")
MEASURE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MEASURE)


class FakeEncoding:
    """A stand-in encoder that divides the character count.

    Deliberately NOT a real tokenizer: these tests must run where tiktoken is
    absent, and more importantly they must be able to assert what happens when
    an encoder is WRONG, which a real tokenizer cannot be made to do on demand.

    `divisor` 1 makes it agree with nothing real; a large divisor makes it
    collapse toward zero, which is exactly the degenerate behaviour a silent
    character-count fallback produces.
    """

    def __init__(self, divisor: int = 1):
        self.divisor = divisor
        self.seen: list[str] = []

    def encode(self, text: str) -> list[int]:
        self.seen.append(text)
        return [0] * (len(text) // self.divisor)


class FakeTiktoken(types.ModuleType):
    """Minimal stand-in for the `tiktoken` module, installed into sys.modules."""

    def __init__(self):
        super().__init__("tiktoken")
        self.loaded: list[str] = []
        self.registry: dict[str, FakeEncoding] = {}
        self.fail_on: set[str] = set()

    def get_encoding(self, name):
        self.loaded.append(name)
        if name in self.fail_on:
            raise RuntimeError(f"no such encoding: {name}")
        return self.registry.setdefault(name, FakeEncoding())


@contextlib.contextmanager
def installed_fake_tiktoken():
    fake = FakeTiktoken()
    saved = sys.modules.get("tiktoken")
    sys.modules["tiktoken"] = fake
    try:
        yield fake
    finally:
        if saved is None:
            sys.modules.pop("tiktoken", None)
        else:
            sys.modules["tiktoken"] = saved


def run_main(argv) -> tuple[int, str]:
    """Run the script's `main()` and return (exit code, combined output)."""
    saved_argv, saved_stdout, saved_stderr = sys.argv, sys.stdout, sys.stderr
    buffer = io.StringIO()
    sys.argv = ["measure_tokens.py", *argv]
    sys.stdout = sys.stderr = buffer
    try:
        code = MEASURE.main()
    finally:
        sys.argv, sys.stdout, sys.stderr = saved_argv, saved_stdout, saved_stderr
    return code, buffer.getvalue()


class EncodingHonestyTest(unittest.TestCase):
    def test_a_reference_case_mismatch_refuses_the_whole_measurement(self):
        """A miscounting encoder must not contribute a plausible number."""
        with installed_fake_tiktoken() as fake:
            # 11 characters // 100 == 0, against a published reference of 2.
            fake.registry["cl100k_base"] = FakeEncoding(divisor=100)
            with self.assertRaises(MEASURE.MeasurementError) as caught:
                MEASURE.load_encodings(("cl100k_base",))
        self.assertIn("not the encoder it claims to be", str(caught.exception))

    def test_a_failed_load_is_not_degraded_into_a_measurement(self):
        """No tokenizer means no token figure — not a character-count stand-in."""
        with installed_fake_tiktoken() as fake:
            fake.fail_on.add("cl100k_base")
            with self.assertRaises(MEASURE.MeasurementError) as caught:
                MEASURE.load_encodings(("cl100k_base",))
        self.assertIn("no such encoding", str(caught.exception))

    def test_missing_tiktoken_names_the_alternative_rather_than_guessing(self):
        """Without the dependency the honest output is the BYTE figure, named."""
        saved = sys.modules.pop("tiktoken", None)
        sys.modules["tiktoken"] = None  # import finds it, but it is not a module
        try:
            with self.assertRaises(MEASURE.MeasurementError) as caught:
                MEASURE.load_encodings(("cl100k_base",))
        finally:
            if saved is None:
                sys.modules.pop("tiktoken", None)
            else:
                sys.modules["tiktoken"] = saved
        self.assertIn("install tiktoken", str(caught.exception))

    def test_two_identical_encodings_are_reported_as_one_claim_not_two(self):
        """Loading one vocabulary twice is not two independent tokenizers."""
        shared = FakeEncoding()
        encodings = {"cl100k_base": shared, "o200k_base": shared}
        with self.assertRaises(MEASURE.MeasurementError) as caught:
            MEASURE.prove_encodings_differ(encodings)
        self.assertIn("not distinct tokenizers", str(caught.exception))

    def test_genuinely_distinct_encodings_pass_the_cross_check(self):
        """The check can also succeed, or it proves nothing by construction."""
        encodings = {"cl100k_base": FakeEncoding(4), "o200k_base": FakeEncoding(7)}
        note = MEASURE.prove_encodings_differ(encodings)
        self.assertIn("2 encodings agree on being distinct", note)

    def test_a_single_encoding_states_that_no_cross_check_applies(self):
        """One encoding is a legitimate measurement; it just cannot self-check."""
        note = MEASURE.prove_encodings_differ({"cl100k_base": FakeEncoding()})
        self.assertIn("no cross-check applicable", note)


class SavingHonestyTest(unittest.TestCase):
    def test_declined_calls_must_cost_the_same_in_both_directions(self):
        """A 'full' row whose two documents differ is not describing real behaviour.

        This is the check that keeps the saving honest. A declined compact
        request returns the full value, so the two columns MUST agree for those
        rows. If a dump ever claims otherwise, the totals are measuring
        something the release does not do.
        """
        rows = [
            {
                "label": "declined",
                "plan": "full:no_transforms",
                "full": "a" * 40,
                "adaptive": "a" * 20,  # a declined request did NOT shrink
            }
        ]
        with self.assertRaises(MEASURE.MeasurementError) as caught:
            MEASURE.measure(rows, {"fake": FakeEncoding()})
        message = str(caught.exception)
        self.assertIn("does not match the behaviour it is measuring", message)
        self.assertIn("1 full:no_transforms calls", message)

    def test_the_total_counts_declined_calls_rather_than_only_the_winners(self):
        """A saving that ignores the 81 declined calls describes nobody's release."""
        rows = [
            {"label": "compacted", "plan": "compact", "full": "a" * 100, "adaptive": "a" * 50},
            {
                "label": "declined",
                "plan": "full:no_transforms",
                "full": "b" * 100,
                "adaptive": "b" * 100,
            },
        ]
        report = MEASURE.measure(rows, {"fake": FakeEncoding()})["fake"]

        # 200 -> 150 across the whole set. The winner alone would read 100 -> 50
        # and report a 50% saving the release does not deliver.
        self.assertEqual(report["full_tokens"], 200)
        self.assertEqual(report["adaptive_tokens"], 150)
        self.assertEqual(report["tokens_saved"], 50)
        self.assertEqual(report["percent_saved"], 25.0)
        self.assertEqual(report["calls"], 2)

        # The breakdown has to expose the split, so a reader can see the saving
        # comes entirely from the compacted calls and none from the declined ones.
        self.assertEqual(report["per_plan"]["compact"]["calls"], 1)
        self.assertAlmostEqual(report["per_plan"]["compact"]["percent_saved"], 50.0)
        self.assertAlmostEqual(report["per_plan"]["full:no_transforms"]["percent_saved"], 0.0)

    def test_an_empty_document_set_reports_no_percentage_rather_than_dividing(self):
        """Zero tokens in, zero tokens out: the saving is undefined, not infinite."""
        rows = [{"label": "empty", "plan": "compact", "full": "", "adaptive": ""}]
        report = MEASURE.measure(rows, {"fake": FakeEncoding()})["fake"]
        self.assertEqual(report["full_tokens"], 0)
        self.assertEqual(report["tokens_saved"], 0)
        self.assertIsNone(report["percent_saved"])
        # The per-plan figure is undefined for the same reason.
        self.assertIsNone(report["per_plan"]["compact"]["percent_saved"])

    def test_main_reports_an_empty_dump_as_undefined_rather_than_zero_percent(self):
        """End to end: documents that tokenise to nothing must not print 0.00%.

        0.00% would read as "measured, saved nothing" and mean "there was
        nothing to measure". Those are the same distinction the rest of this
        work keeps, so the report keeps it.
        """
        with tempfile.TemporaryDirectory() as name:
            dump = Path(name) / "dump.json"
            rows = [{"label": f"e{i}", "plan": "compact", "full": "", "adaptive": ""} for i in range(3)]
            dump.write_text(json.dumps(rows))
            with installed_fake_tiktoken() as fake:
                # Both must reproduce the published "hello world" -> 2 reference
                # (11 // 4 == 11 // 5 == 2) while disagreeing on longer text, or
                # the script's own distinctness check correctly refuses to treat
                # them as two independent tokenizers.
                fake.registry["cl100k_base"] = FakeEncoding(divisor=4)
                fake.registry["o200k_base"] = FakeEncoding(divisor=5)
                code, output = run_main([str(dump)])
        self.assertEqual(code, 0)
        self.assertIn("3 inspect documents", output)
        self.assertIn("undefined", output)
        self.assertNotIn("0.00%", output)


class DumpValidationTest(unittest.TestCase):
    def test_a_missing_dump_names_the_command_that_produces_one(self):
        with tempfile.TemporaryDirectory() as name:
            code, output = run_main([str(Path(name) / "nope.json")])
        self.assertEqual(code, 2)
        self.assertIn("is not a file", output)
        self.assertIn("TDMCP_TOKEN_DUMP", output)

    def test_a_dump_holding_nothing_is_an_error_not_a_zero_saving(self):
        """'No measurement' and 'measured, no saving' must stay different answers."""
        with tempfile.TemporaryDirectory() as name:
            dump = Path(name) / "empty.json"
            dump.write_text(json.dumps([]))
            code, output = run_main([str(dump)])
        self.assertEqual(code, 2)
        self.assertIn("no document pairs", output)

    def test_a_dump_that_is_not_a_list_is_refused_rather_than_iterated(self):
        """A truncated or hand-edited dump must not produce a plausible partial total."""
        with tempfile.TemporaryDirectory() as name:
            dump = Path(name) / "object.json"
            dump.write_text(json.dumps({"label": "one row", "plan": "compact"}))
            code, output = run_main([str(dump)])
        self.assertEqual(code, 2)
        self.assertIn("no document pairs", output)

    def test_a_refused_measurement_exits_non_zero_so_no_number_can_be_quoted(self):
        """The script's exit status is the guarantee that a report was earned."""
        with tempfile.TemporaryDirectory() as name:
            dump = Path(name) / "dump.json"
            rows = [
                {
                    "label": "declined",
                    "plan": "full:not_smaller",
                    "full": "a" * 40,
                    "adaptive": "a" * 10,
                }
            ]
            dump.write_text(json.dumps(rows))
            with installed_fake_tiktoken() as fake:
                for name in MEASURE.DEFAULT_ENCODINGS:
                    fake.registry[name] = FakeEncoding(divisor=5)
                code, output = run_main([str(dump)])
        self.assertEqual(code, 1)
        self.assertIn("measurement refused", output)
        # Nothing that looks like a percentage may survive a refused measurement.
        self.assertNotIn("%\n", output.split("measurement refused")[-1].replace("0.00%", ""))


if __name__ == "__main__":
    unittest.main()
