"""Unit tests for exact numerical sampling (no live TD).

Every fact pinned here was measured against TouchDesigner 2025.32460; each test
names the measurement it reproduces. The fakes are plain Python rather than
numpy arrays on purpose: `numpyArray()` hands back a real numpy array, but the
bridge code only ever *indexes* it. Indexing-only fakes prove that, and they
also let the suite run on a host with no numpy -- which is the actual situation
here, and which a numpy-based fake would have hidden.

Measured facts these encode:
  * `numpyArray()` is float32 of shape [H, W, C]: row-first, the opposite of the
    usual [w, h] image convention.
  * The array dtype says nothing about range. Under `rgba8fixed` a value of 4.5
    reads back as 1.0; only `rgba32float` carries it.
  * RGB is premultiplied by alpha by default on constantTOP, so 4.5 at alpha
    0.25 reads back as 1.125.
  * `sample(u=, v=)` is a separate interpolated/border lookup, NOT the data
    path; positional `sample(0, 0)` is rejected.
  * Assigning an invalid format string to `par.format` is silently ignored, so
    the only honest signal is `pixelFormatName` read back after the fact.
"""

from __future__ import annotations

import json
import os
import sys
import unittest
from types import SimpleNamespace
from typing import Any
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402
from tdmcp_bridge import sample_data  # noqa: E402

NAN = float("nan")
INF = float("inf")


class FakeArray:
    """A `[H, W, C]` stand-in supporting only the indexing the bridge uses."""

    def __init__(self, rows: list[list[list[float]]], dtype: str = "float32") -> None:
        self._rows = rows
        self.dtype = dtype
        self.shape = (len(rows), len(rows[0]), len(rows[0][0]))

    def __getitem__(self, index: Any) -> Any:
        if isinstance(index, tuple):
            y, x = index
            return self._rows[y][x]
        return self._rows[index]

    def __len__(self) -> int:
        return len(self._rows)


def ramp(rows: int = 4, cols: int = 4) -> FakeArray:
    """R == column, G == row, so any axis transpose is visible."""
    return FakeArray(
        [[[x / cols, y / rows, 0.0, 1.0] for x in range(cols)] for y in range(rows)]
    )


def fake_top(
    array: FakeArray,
    *,
    pixel_format: str = "rgba32float",
    premult: bool | None = None,
    family: str = "TOP",
    path: str = "/project1/probe/px",
    width: int | None = None,
    height: int | None = None,
    cook_error: Exception | None = None,
    expose_numpy: bool = True,
) -> SimpleNamespace:
    """A TOP double whose `numpyArray()` returns `array` verbatim."""

    def cook(force: bool = False) -> None:
        if cook_error is not None:
            raise cook_error

    node = SimpleNamespace(
        family=family,
        path=path,
        name="px",
        valid=True,
        width=array.shape[1] if width is None else width,
        height=array.shape[0] if height is None else height,
        pixelFormatName=pixel_format,
        cook=cook,
        par=SimpleNamespace(premultrgbbyalpha=SimpleNamespace(val=premult)),
        cookedThisFrame=1,
        cookedPreviousFrame=1,
        cookFrame=1,
    )
    if expose_numpy:
        node.numpyArray = lambda: array
    return node


class ExactPixelTests(unittest.TestCase):
    def test_a_named_pixel_reads_exact_values(self) -> None:
        out = sample_data._sample_top(
            fake_top(ramp()), "/project1/probe/px", {"pixels": [[0, 0], [3, 0]]}
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["mode"], "exact_pixels")
        self.assertEqual(out["requested"], 2)
        self.assertEqual(out["returned"], 2)
        self.assertEqual(out["outOfRange"], 0)
        self.assertEqual(out["pixels"][0]["x"], 0)
        self.assertEqual(out["pixels"][0]["y"], 0)

    def test_the_xy_to_hw_swap_is_explicit(self) -> None:
        # The measured trap: the array is [H, W, C]. A caller naming the pixel
        # at x=3,y=1 must get row 1 column 3, not row 3 column 1.
        array = ramp()
        out = sample_data._sample_top(fake_top(array), "/p", {"pixels": [[3, 1]]})
        entry = out["pixels"][0]
        self.assertEqual((entry["y"], entry["x"]), (1, 3))
        for got, want in zip(entry["values"], array[1, 3]):
            self.assertAlmostEqual(got, want, places=6)

    def test_array_order_is_reported_because_it_is_the_easy_mistake(self) -> None:
        out = sample_data._sample_top(fake_top(ramp()), "/p", {"pixels": [[0, 0]]})
        self.assertEqual(out["arrayOrder"], "row_major_hwc")
        self.assertEqual(list(out["arrayShape"]), [4, 4, 4])

    def test_an_out_of_range_pixel_is_counted_not_clamped(self) -> None:
        # Clamping would silently answer a different question and make "how much
        # of my request was bad" unknowable.
        out = sample_data._sample_top(
            fake_top(ramp()), "/p", {"pixels": [[0, 0], [99, 0], [-1, 0]]}
        )
        self.assertEqual(out["returned"], 1)
        self.assertEqual(out["outOfRange"], 2)
        self.assertEqual(out["requested"], 3)

    def test_a_malformed_pixel_spec_is_out_of_range_not_a_crash(self) -> None:
        out = sample_data._sample_top(
            fake_top(ramp()), "/p", {"pixels": [[0, 0], [1], "x", None, [0, 0, 0]]}
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["outOfRange"], 4)

    def test_too_many_pixels_is_refused(self) -> None:
        from tdmcp_bridge import constants as C

        out = sample_data._sample_top(
            fake_top(ramp()),
            "/p",
            {"pixels": [[0, 0]] * (C.SAMPLE_MAX_POINT_SAMPLES + 1)},
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.sample.too_many_pixels")


class NonFiniteTests(unittest.TestCase):
    def test_a_nan_is_null_and_counted(self) -> None:
        # JSON has no NaN. Python would emit the bare token and a strict parser
        # would reject the WHOLE response, so one bad pixel would destroy an
        # otherwise good read.
        array = ramp()
        array[0][0][0] = NAN
        array[1][0][1] = INF
        out = sample_data._sample_top(fake_top(array), "/p", {"pixels": [[0, 0], [0, 1]]})
        self.assertTrue(out["ok"])
        self.assertIsNone(out["pixels"][0]["values"][0])
        self.assertIsNone(out["pixels"][1]["values"][1])
        self.assertEqual(out["nonFinite"]["nan"], 1)
        self.assertEqual(out["nonFinite"]["inf"], 1)

    def test_the_response_is_strict_json(self) -> None:
        array = ramp()
        array[0][0][0] = NAN
        out = sample_data._sample_top(fake_top(array), "/p", {"pixels": [[0, 0]]})
        # `allow_nan=False` makes Python raise instead of emitting a bare NaN,
        # which is the property a strict consumer depends on.
        text = json.dumps(out, allow_nan=False)
        self.assertNotIn("NaN", text)
        self.assertIn("null", text)


class RegionTests(unittest.TestCase):
    def test_a_region_is_row_major_over_the_rectangle(self) -> None:
        out = sample_data._sample_top(
            fake_top(ramp()), "/p", {"region": {"x": 0, "y": 0, "x1": 2, "y1": 2}}
        )
        self.assertTrue(out["ok"])
        self.assertEqual(out["mode"], "exact_region")
        self.assertEqual(out["pixelCount"], 4)
        self.assertEqual(out["valuesPerPixel"], 4)
        self.assertEqual(len(out["values"]), 16)
        self.assertAlmostEqual(out["values"][0], 0.0, places=6)
        self.assertAlmostEqual(out["values"][4], 0.25, places=6)

    def test_an_inverted_or_outside_region_is_refused(self) -> None:
        node = fake_top(ramp())
        for bad in [
            {"x": 0, "y": 0, "x1": 0, "y1": 2},
            {"x": 0, "y": 0, "x1": 2, "y1": 0},
            {"x": 0, "y": 0, "x1": 99, "y1": 2},
        ]:
            out = sample_data._sample_top(node, "/p", {"region": bad})
            self.assertFalse(out["ok"], bad)
            self.assertEqual(out["code"], "tdmcp.sample.bad_region", bad)

    def test_an_oversized_region_is_refused_before_serializing(self) -> None:
        from tdmcp_bridge import constants as C

        # Large enough to be in bounds and past the cap, so the size refusal is
        # what fires. A region that is also out of bounds is a DIFFERENT
        # refusal, covered above.
        big = ramp(600, 600)
        out = sample_data._sample_top(
            fake_top(big), "/p", {"region": {"x": 0, "y": 0, "x1": 600, "y1": 600}}
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.sample.region_too_large")
        self.assertEqual(C.SAMPLE_MAX_REGION_PIXELS, 262144)

    def test_an_out_of_bounds_region_is_refused_before_the_size_cap(self) -> None:
        # Order matters: a caller who asked for the wrong place should be told
        # that, not that the request was too big.
        out = sample_data._sample_top(
            fake_top(ramp()), "/p", {"region": {"x": 0, "y": 0, "x1": 64, "y1": 64}}
        )
        self.assertEqual(out["code"], "tdmcp.sample.bad_region")

    def test_a_missing_bound_is_a_bad_region(self) -> None:
        out = sample_data._sample_top(fake_top(ramp()), "/p", {"region": {"x": 0, "y": 0}})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.sample.bad_region")


class SummaryTests(unittest.TestCase):
    def test_a_summary_is_labelled_as_a_summary(self) -> None:
        # A summary is not the data. A caller that cannot tell them apart will
        # compare statistics and call it an image comparison.
        out = sample_data._sample_top(fake_top(ramp()), "/p", {})
        self.assertTrue(out["ok"])
        self.assertEqual(out["mode"], "summary")
        self.assertTrue(out["isSummary"])
        self.assertIn("not the image's contents", out["explanation"])
        self.assertNotIn("values", out)
        self.assertEqual(out["pixelCount"], 16)
        self.assertEqual(out["valueCount"], 64)
        self.assertAlmostEqual(out["min"], 0.0, places=6)
        # The ramp's alpha channel is 1.0, so the image max is 1.0 -- a summary
        # that ignored alpha would be answering a different question.
        self.assertAlmostEqual(out["max"], 1.0, places=6)

    def test_a_summary_counts_non_finite_rather_than_averaging_them(self) -> None:
        array = ramp()
        array[0][0][0] = NAN
        out = sample_data._sample_top(fake_top(array), "/p", {})
        self.assertEqual(out["nonFinite"]["nan"], 1)
        self.assertEqual(out["finiteCount"], 63)
        self.assertIsNotNone(out["mean"])

    def test_an_entirely_non_finite_image_is_not_scored_as_zero(self) -> None:
        array = FakeArray([[[NAN] * 4, [NAN] * 4], [[NAN] * 4, [NAN] * 4]])
        out = sample_data._sample_top(fake_top(array), "/p", {})
        self.assertTrue(out["ok"])
        self.assertIsNone(out["mean"])
        self.assertIsNone(out["min"])
        self.assertIsNone(out["max"])
        self.assertTrue(out["allNonFinite"])
        self.assertEqual(out["finiteCount"], 0)

    def test_an_hr_value_survives_a_summary_in_a_float_format(self) -> None:
        # Measured under rgba32float: 4.5 at alpha 0.25, premultiplied, is 1.125.
        array = FakeArray([[[1.125, 0.5625, 0.125, 0.25]]])
        out = sample_data._sample_top(fake_top(array), "/p", {})
        self.assertAlmostEqual(out["max"], 1.125, places=6)
        self.assertTrue(out["format"]["carriesHdr"])

    def test_a_summary_ignores_the_requested_pixels(self) -> None:
        # Asking for pixels AND getting a summary would be a silent
        # substitution of a cheaper answer for the one that was requested.
        out = sample_data._sample_top(fake_top(ramp()), "/p", {"pixels": [[0, 0]]})
        self.assertEqual(out["mode"], "exact_pixels")


class FormatAndFreshnessTests(unittest.TestCase):
    def test_a_float_format_is_reported_as_carrying_hdr(self) -> None:
        out = sample_data._sample_top(
            fake_top(ramp(), pixel_format="rgba32float"), "/p", {"pixels": [[0, 0]]}
        )
        self.assertEqual(out["format"]["pixelFormat"], "rgba32float")
        self.assertTrue(out["format"]["carriesHdr"])

    def test_an_eight_bit_format_does_not_claim_hdr_despite_a_float32_array(self) -> None:
        # Measured: numpyArray is ALWAYS float32, and under rgba8fixed a value
        # of 4.5 reads back as 1.0. Trusting the dtype would report clamped data
        # as exact.
        out = sample_data._sample_top(
            fake_top(ramp(), pixel_format="rgba8fixed"), "/p", {"pixels": [[0, 0]]}
        )
        self.assertEqual(out["arrayDtype"], "float32")
        self.assertFalse(out["format"]["carriesHdr"])
        self.assertIn("not from the numpy dtype", out["format"]["note"])

    def test_premultiplied_alpha_is_reported_when_the_op_declares_it(self) -> None:
        out = sample_data._sample_top(
            fake_top(ramp(), premult=True), "/p", {"pixels": [[0, 0]]}
        )
        self.assertTrue(out["premultipliedByAlpha"])
        out = sample_data._sample_top(fake_top(ramp()), "/p", {"pixels": [[0, 0]]})
        self.assertIsNone(out["premultipliedByAlpha"])

    def test_forcing_a_cook_cooks_before_reading_and_reports_it(self) -> None:
        order: list[str] = []
        array = ramp()
        node = fake_top(array)
        node.cook = lambda force=False: order.append("cook")
        node.numpyArray = lambda: (order.append("read"), array)[1]
        out = sample_data._sample_top(
            node, "/p", {"pixels": [[0, 0]], "forceCook": True}
        )
        self.assertEqual(order, ["cook", "read"])
        self.assertTrue(out["cook"]["forced"])

    def test_without_force_the_read_does_not_cook(self) -> None:
        order: list[str] = []
        array = ramp()
        node = fake_top(array)
        node.cook = lambda force=False: order.append("cook")
        node.numpyArray = lambda: (order.append("read"), array)[1]
        out = sample_data._sample_top(node, "/p", {"pixels": [[0, 0]]})
        self.assertEqual(order, ["read"])
        self.assertFalse(out["cook"]["forced"])

    def test_a_failed_forced_cook_is_an_error_not_a_stale_read(self) -> None:
        node = fake_top(ramp(), cook_error=RuntimeError("boom"))
        out = sample_data._sample_top(node, "/p", {"pixels": [[0, 0]], "forceCook": True})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.sample.cook_failed")


class UnsupportedTests(unittest.TestCase):
    def test_a_build_without_numpyarray_reports_unavailable_not_8_bit_data(self) -> None:
        # saveByteArray would "work" here and return clamped 8-bit values. Saying
        # so is the point: the alternative looks like a successful exact read.
        node = fake_top(ramp(), expose_numpy=False)
        out = sample_data._sample_top(node, "/p", {"pixels": [[0, 0]]})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.sample.unsupported")
        self.assertIn("8-bit", out["message"])

    def test_a_non_image_array_is_refused_rather_than_reshaped(self) -> None:
        flat = SimpleNamespace(
            shape=(16,), dtype="float32", __getitem__=lambda _i: 0.0
        )
        node = fake_top(ramp())
        node.numpyArray = lambda: flat
        out = sample_data._sample_top(node, "/p", {"pixels": [[0, 0]]})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.sample.unsupported")

    def test_a_zero_resolution_top_is_refused(self) -> None:
        node = fake_top(ramp(), width=0, height=0)
        out = sample_data._sample_top(node, "/p", {"pixels": [[0, 0]]})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.sample.empty")

    def test_a_numpyarray_that_raises_is_unsupported_not_a_crash(self) -> None:
        node = fake_top(ramp())

        def boom() -> Any:
            raise RuntimeError("texture gone")

        node.numpyArray = boom
        out = sample_data._sample_top(node, "/p", {"pixels": [[0, 0]]})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.sample.unsupported")


class FakeChannel:
    def __init__(self, samples: list[float]) -> None:
        self._samples = samples

    def __getitem__(self, index: int) -> float:
        return self._samples[index]


def fake_chop(
    *, num_chans: int, num_samples: int, samples: list[list[float]] | None = None,
    family: str = "CHOP", expose_numpy: bool = True,
) -> SimpleNamespace:
    """A CHOP double. `samples[c][s]` is channel c, sample s."""
    if samples is None:
        samples = [[0.0] * num_samples for _ in range(num_chans)]
    chans = [FakeChannel(row) for row in samples]
    node = SimpleNamespace(
        family=family,
        path="/project1/probe/ch",
        name="ch",
        valid=True,
        numChans=num_chans,
        numSamples=num_samples,
        chans=chans,
        cook=lambda force=False: None,
    )
    if expose_numpy:
        # Deliberately transposed [samples, chans], the layout the numpy branch
        # reads. Its axis order is unverified on this build, which is why the
        # indexer is the default.
        node.numpyArray = lambda: FakeChopArray(samples)
    return node


class FakeChopArray:
    def __init__(self, samples: list[list[float]]) -> None:
        self._samples = samples
        self.shape = (len(samples[0]) if samples else 0, len(samples))
        self.dtype = "float32"

    def __getitem__(self, index: int) -> list[float]:
        return [row[index] for row in self._samples]


class ChopTests(unittest.TestCase):
    def test_exact_samples_come_back_per_channel(self) -> None:
        node = fake_chop(
            num_chans=2, num_samples=3, samples=[[1.0, 3.5, 2.0], [2.0, -4.25, 1.0]]
        )
        out = sample_data._sample_chop(node, "/project1/probe/ch", {})
        self.assertTrue(out["ok"])
        self.assertEqual(out["mode"], "exact_samples")
        self.assertEqual(out["returnedChans"], 2)
        self.assertEqual(out["returnedSamples"], 3)
        self.assertFalse(out["truncated"])
        self.assertEqual(out["channels"][0]["samples"], [1.0, 3.5, 2.0])
        # A negative value survives: a CHOP is not clamped to [0, 1].
        self.assertEqual(out["channels"][1]["samples"][1], -4.25)

    def test_the_indexed_read_is_the_default_because_it_is_the_measured_one(self) -> None:
        # numpyArray's axis order on a CHOP was never measured; the indexer was.
        node = fake_chop(num_chans=1, num_samples=2, samples=[[1.0, 2.0]])
        out = sample_data._sample_chop(node, "/p", {})
        self.assertEqual(out["readMethod"], "indexed")
        self.assertTrue(out["axisOrderVerified"])
        self.assertEqual(out["channels"][0]["samples"], [1.0, 2.0])

    def test_the_numpy_read_reports_its_unverified_axis_order(self) -> None:
        node = fake_chop(num_chans=2, num_samples=2, samples=[[1.0, 2.0], [3.0, 4.0]])
        out = sample_data._sample_chop(node, "/p", {"readMethod": "numpyArray"})
        self.assertTrue(out["ok"])
        self.assertEqual(out["readMethod"], "numpyArray")
        self.assertFalse(
            out["axisOrderVerified"],
            "an unmeasured axis order must not be presented as verified",
        )
        self.assertEqual(list(out["arrayShape"]), [2, 2])

    def test_a_nan_in_a_chop_is_null_and_counted(self) -> None:
        node = fake_chop(num_chans=1, num_samples=2, samples=[[1.0, NAN]])
        out = sample_data._sample_chop(node, "/p", {})
        self.assertIsNone(out["channels"][0]["samples"][1])
        self.assertEqual(out["channels"][0]["nonFinite"]["nan"], 1)

    def test_truncation_is_reported_rather_than_silent(self) -> None:
        from tdmcp_bridge import constants as C

        total = C.SAMPLE_DATA_MAX_SCALARS + 10
        node = fake_chop(num_chans=1, num_samples=total, samples=[[0.0] * total])
        out = sample_data._sample_chop(node, "/p", {})
        self.assertTrue(out["ok"])
        self.assertTrue(out["truncated"])
        self.assertEqual(out["numSamples"], total)
        self.assertLess(out["returnedSamples"], total)

    def test_an_empty_chop_is_refused(self) -> None:
        node = fake_chop(num_chans=0, num_samples=0, samples=[])
        out = sample_data._sample_chop(node, "/p", {})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.sample.empty")

    def test_a_failed_forced_cook_is_an_error(self) -> None:
        node = fake_chop(num_chans=1, num_samples=1, samples=[[1.0]])

        def boom(force: bool = False) -> None:
            raise RuntimeError("no")

        node.cook = boom
        out = sample_data._sample_chop(node, "/p", {"forceCook": True})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.sample.cook_failed")

    def test_an_unreadable_channel_is_reported_not_partially_returned(self) -> None:
        class Broken:
            def __getitem__(self, index: int) -> float:
                raise RuntimeError("channel gone")

        node = fake_chop(num_chans=1, num_samples=1, samples=[[1.0]])
        node.chans = [Broken()]
        out = sample_data._sample_chop(node, "/p", {})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.sample.read_failed")


class DispatchTests(unittest.TestCase):
    def test_sample_data_is_a_registered_route(self) -> None:
        # A capability is claimed from a route that exists.
        self.assertIn("sample_data", tdmcp_bridge.HANDLERS)
        self.assertIs(
            tdmcp_bridge.HANDLERS["sample_data"], sample_data.handle_sample_data
        )

    def test_a_comp_is_refused_as_a_family(self) -> None:
        node = fake_top(ramp(), family="COMP")
        with mock.patch.object(sample_data, "resolve_op", return_value=node):
            out = sample_data.handle_sample_data({"path": "/p"})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.sample.unsupported_family")
        self.assertEqual(out["family"], "COMP")

    def test_an_unresolvable_path_is_not_found(self) -> None:
        # `resolve_op` is the real one, so it needs the `td` module; returning
        # None is what it does for a path that does not resolve.
        with mock.patch.object(sample_data, "resolve_op", return_value=None):
            out = sample_data.handle_sample_data({"path": "/project1/definitely_missing"})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.op.not_found")

    def test_an_invalid_op_is_not_found(self) -> None:
        node = fake_top(ramp())
        node.valid = False
        with mock.patch.object(sample_data, "resolve_op", return_value=node):
            out = sample_data.handle_sample_data({"path": "/p"})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.op.not_found")

    def test_a_chop_routes_to_the_chop_reader(self) -> None:
        node = fake_chop(num_chans=1, num_samples=2, samples=[[1.0, 2.0]])
        with mock.patch.object(sample_data, "resolve_op", return_value=node):
            out = sample_data.handle_sample_data({"path": "/p"})
        self.assertTrue(out["ok"])
        self.assertEqual(out["family"], "CHOP")
        self.assertEqual(out["mode"], "exact_samples")


if __name__ == "__main__":
    unittest.main()
