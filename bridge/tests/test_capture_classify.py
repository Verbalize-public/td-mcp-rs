"""Unit tests for capture black / uniform frame classification (no live TD)."""

from __future__ import annotations

import os
import sys
import unittest
from types import SimpleNamespace

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402


class FakeNdarray:
    """Minimal ndarray stand-in for `_classify_frame` (mean / max / min / slice)."""

    def __init__(self, planes: list[list[list[float]]]):
        # planes: H x W x C
        self._planes = planes
        h = len(planes)
        w = len(planes[0]) if h else 0
        c = len(planes[0][0]) if h and w else 0
        self.shape = (h, w, c)
        self.ndim = 3
        self.size = h * w * c

    def __getitem__(self, key: object) -> FakeNdarray:
        if (
            isinstance(key, tuple)
            and len(key) == 2
            and key[0] is Ellipsis
            and isinstance(key[1], slice)
        ):
            stop = key[1].stop if key[1].stop is not None else self.shape[2]
            start = key[1].start or 0
            sliced = [
                [[px[c] for c in range(start, stop)] for px in row]
                for row in self._planes
            ]
            return FakeNdarray(sliced)
        raise TypeError(f"unsupported index: {key!r}")

    def mean(self, axis: object | None = None) -> float | list[float]:
        vals = [c for row in self._planes for px in row for c in px]
        if axis is None:
            return sum(vals) / len(vals)
        if axis == (0, 1):
            return self._reduce_channels(lambda ch: sum(ch) / len(ch))
        raise TypeError(f"unsupported axis: {axis!r}")

    def max(self, axis: object | None = None) -> float | list[float]:
        if axis is None:
            return max(c for row in self._planes for px in row for c in px)
        if axis == (0, 1):
            return self._reduce_channels(max)
        raise TypeError(f"unsupported axis: {axis!r}")

    def min(self, axis: object | None = None) -> float | list[float]:
        if axis is None:
            return min(c for row in self._planes for px in row for c in px)
        if axis == (0, 1):
            return self._reduce_channels(min)
        raise TypeError(f"unsupported axis: {axis!r}")

    def _reduce_channels(self, fn: object) -> list[float]:
        c = self.shape[2]
        out: list[float] = []
        for ci in range(c):
            ch = [px[ci] for row in self._planes for px in row]
            out.append(float(fn(ch)))  # type: ignore[operator]
        return out


def _solid(rgb: tuple[float, float, float], *, h: int = 2, w: int = 2) -> FakeNdarray:
    return FakeNdarray([[list(rgb) for _ in range(w)] for _ in range(h)])


def _gradient() -> FakeNdarray:
    return FakeNdarray(
        [
            [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
            [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
        ]
    )


def _top(arr: FakeNdarray | None) -> SimpleNamespace:
    if arr is None:
        return SimpleNamespace()  # no numpyArray
    return SimpleNamespace(numpyArray=lambda delayed=False: arr)


class CaptureClassifyTest(unittest.TestCase):
    def test_black_solid(self) -> None:
        kind, mean = tdmcp_bridge._classify_frame(_top(_solid((0.0, 0.0, 0.0))), b"x" * 500)
        self.assertEqual(kind, "black")
        self.assertIsNotNone(mean)
        assert mean is not None
        self.assertAlmostEqual(mean[0], 0.0)

    def test_uniform_white(self) -> None:
        kind, mean = tdmcp_bridge._classify_frame(_top(_solid((1.0, 1.0, 1.0))), b"x" * 500)
        self.assertEqual(kind, "uniform")
        assert mean is not None
        self.assertAlmostEqual(mean[0], 1.0)

    def test_uniform_red(self) -> None:
        kind, mean = tdmcp_bridge._classify_frame(_top(_solid((1.0, 0.0, 0.0))), b"x" * 500)
        self.assertEqual(kind, "uniform")
        assert mean is not None
        self.assertAlmostEqual(mean[0], 1.0)
        self.assertAlmostEqual(mean[1], 0.0)

    def test_non_uniform_ok(self) -> None:
        kind, _mean = tdmcp_bridge._classify_frame(_top(_gradient()), b"x" * 500)
        self.assertIsNone(kind)

    def test_fallback_tiny_image_is_black(self) -> None:
        kind, mean = tdmcp_bridge._classify_frame(_top(None), b"tiny")
        self.assertEqual(kind, "black")
        self.assertIsNone(mean)

    def test_fallback_large_image_ok(self) -> None:
        kind, mean = tdmcp_bridge._classify_frame(_top(None), b"x" * 500)
        self.assertIsNone(kind)
        self.assertIsNone(mean)

    def test_perception_messages(self) -> None:
        black = tdmcp_bridge._perception_frame_message("black", (0.0, 0.0, 0.0))
        self.assertIn("black", black.lower())
        self.assertIn("0.00,0.00,0.00", black)
        uni = tdmcp_bridge._perception_frame_message("uniform", (1.0, 0.0, 0.0))
        self.assertIn("uniform", uni.lower())
        self.assertIn("1.00,0.00,0.00", uni)


if __name__ == "__main__":
    unittest.main()
