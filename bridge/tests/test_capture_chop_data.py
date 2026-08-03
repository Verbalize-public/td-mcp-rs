"""Unit tests for capture chop_data shaping (no live TD)."""

from __future__ import annotations

import os
import sys
import unittest
from types import SimpleNamespace
from typing import Any
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402


class FakeChan:
    def __init__(self, name: str, samples: list[float]):
        self.name = name
        self._samples = samples

    def __getitem__(self, i: int) -> float:
        return self._samples[i]


def _fake_chop(
    *,
    num_chans: int,
    num_samples: int,
    rate: float | None = 60.0,
    family: str = "CHOP",
    path: str = "/project1/zone/const1",
    fill: float = 0.5,
) -> SimpleNamespace:
    chans = [
        FakeChan(f"c{i}", [fill + i * 0.01 + j * 0.001 for j in range(num_samples)])
        for i in range(num_chans)
    ]
    return SimpleNamespace(
        family=family,
        path=path,
        name="const1",
        numChans=num_chans,
        numSamples=num_samples,
        rate=rate,
        chans=chans,
        cook=lambda force=False: None,
        valid=True,
    )


class CaptureChopDataTests(unittest.TestCase):
    def test_happy_path_small_chop(self) -> None:
        node = _fake_chop(num_chans=2, num_samples=4, rate=30.0)
        out = tdmcp_bridge._capture_chop_data(node, "/project1/zone/const1")
        self.assertTrue(out["ok"])
        self.assertEqual(out["mode"], "chop_data")
        self.assertEqual(out["family"], "CHOP")
        self.assertEqual(out["numChans"], 2)
        self.assertEqual(out["numSamples"], 4)
        self.assertEqual(out["rate"], 30.0)
        self.assertEqual(len(out["channels"]), 2)
        self.assertEqual(out["channels"][0]["name"], "c0")
        self.assertEqual(len(out["channels"][0]["samples"]), 4)
        self.assertNotIn("truncation", out)
        self.assertNotIn("imageBase64", out)
        self.assertNotIn("jpegBase64", out)

    def test_all_zero_is_success(self) -> None:
        node = SimpleNamespace(
            family="CHOP",
            path="/project1/zone/zeros",
            name="zeros",
            numChans=1,
            numSamples=3,
            rate=60.0,
            chans=[FakeChan("z", [0.0, 0.0, 0.0])],
            cook=lambda force=False: None,
            valid=True,
        )
        out = tdmcp_bridge._capture_chop_data(node, node.path)
        self.assertTrue(out["ok"])
        self.assertEqual(out["channels"][0]["samples"], [0.0, 0.0, 0.0])

    def test_empty_chop(self) -> None:
        node = _fake_chop(num_chans=0, num_samples=0)
        out = tdmcp_bridge._capture_chop_data(node, node.path)
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.perception.empty_chop")

    def test_empty_samples(self) -> None:
        node = _fake_chop(num_chans=2, num_samples=0)
        out = tdmcp_bridge._capture_chop_data(node, node.path)
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.perception.empty_chop")

    def test_wrong_family(self) -> None:
        node = _fake_chop(num_chans=1, num_samples=1, family="TOP")
        out = tdmcp_bridge._capture_chop_data(node, node.path)
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.perception.wrong_family")

    def test_truncate_channels(self) -> None:
        n = tdmcp_bridge.CHOP_DATA_MAX_CHANNELS + 5
        node = _fake_chop(num_chans=n, num_samples=2)
        out = tdmcp_bridge._capture_chop_data(node, node.path)
        self.assertTrue(out["ok"])
        self.assertEqual(len(out["channels"]), tdmcp_bridge.CHOP_DATA_MAX_CHANNELS)
        self.assertEqual(out["truncation"]["field"], "channels")
        self.assertEqual(
            out["truncation"]["code"], "tdmcp.perception.chop_truncated"
        )

    def test_truncate_samples(self) -> None:
        n = tdmcp_bridge.CHOP_DATA_MAX_SAMPLES + 10
        node = _fake_chop(num_chans=1, num_samples=n)
        out = tdmcp_bridge._capture_chop_data(node, node.path)
        self.assertTrue(out["ok"])
        self.assertEqual(
            len(out["channels"][0]["samples"]), tdmcp_bridge.CHOP_DATA_MAX_SAMPLES
        )
        self.assertEqual(out["truncation"]["field"], "samples")

    def test_truncate_scalars(self) -> None:
        # 32 chans * 256 samples would be 8192 > 4096 → scalar budget wins.
        node = _fake_chop(
            num_chans=tdmcp_bridge.CHOP_DATA_MAX_CHANNELS,
            num_samples=tdmcp_bridge.CHOP_DATA_MAX_SAMPLES,
        )
        out = tdmcp_bridge._capture_chop_data(node, node.path)
        self.assertTrue(out["ok"])
        total = sum(len(c["samples"]) for c in out["channels"])
        self.assertLessEqual(total, tdmcp_bridge.CHOP_DATA_MAX_SCALARS)
        self.assertIn(out["truncation"]["field"], ("scalars", "samples"))

    def test_rate_omitted_when_none(self) -> None:
        node = _fake_chop(num_chans=1, num_samples=1, rate=None)
        out = tdmcp_bridge._capture_chop_data(node, node.path)
        self.assertTrue(out["ok"])
        self.assertNotIn("rate", out)

    def test_effective_mode_auto_chop(self) -> None:
        node = _fake_chop(num_chans=1, num_samples=1)
        self.assertEqual(
            tdmcp_bridge._effective_capture_mode("auto", node), "chop_data"
        )

    def test_effective_mode_auto_pop(self) -> None:
        node = SimpleNamespace(family="POP")
        self.assertEqual(tdmcp_bridge._effective_capture_mode("auto", node), "preview")

    def test_effective_mode_auto_sop_preview(self) -> None:
        node = SimpleNamespace(family="SOP")
        self.assertEqual(tdmcp_bridge._effective_capture_mode("auto", node), "preview")

    def test_effective_mode_auto_dat_preview(self) -> None:
        node = SimpleNamespace(family="DAT")
        self.assertEqual(tdmcp_bridge._effective_capture_mode("auto", node), "preview")

    def test_handle_capture_dispatch_chop_data(self) -> None:
        node = _fake_chop(num_chans=1, num_samples=2)
        with mock.patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
            out = tdmcp_bridge.handle_capture(
                {"path": node.path, "mode": "chop_data"}
            )
        self.assertTrue(out["ok"])
        self.assertEqual(out["mode"], "chop_data")
        self.assertEqual(len(out["channels"][0]["samples"]), 2)

    def test_handle_capture_auto_routes_chop(self) -> None:
        node = _fake_chop(num_chans=1, num_samples=1)
        with mock.patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
            out = tdmcp_bridge.handle_capture({"path": node.path, "mode": "auto"})
        self.assertTrue(out["ok"])
        self.assertEqual(out["mode"], "chop_data")

    def test_handle_capture_wrong_family_explicit(self) -> None:
        node = SimpleNamespace(
            family="TOP",
            path="/project1/probe",
            name="probe",
            valid=True,
            saveByteArray=lambda ext: b"x" * 300,
            width=16,
            height=16,
            parent=lambda: None,
        )
        # Explicit chop_data on TOP
        top_as_chop = SimpleNamespace(
            family="TOP",
            path="/project1/probe",
            name="probe",
            valid=True,
            numChans=1,
            numSamples=1,
            rate=60.0,
            chans=[FakeChan("x", [1.0])],
            cook=lambda force=False: None,
        )
        with mock.patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=top_as_chop):
            out = tdmcp_bridge.handle_capture(
                {"path": "/project1/probe", "mode": "chop_data"}
            )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.perception.wrong_family")

    def test_handle_capture_top_returns_png(self) -> None:
        class Arr:
            shape = (2, 2, 3)
            ndim = 3
            size = 12

            def mean(self, axis=None):  # noqa: ANN001
                if axis == (0, 1):
                    return [0.2, 0.5, 0.8]
                return 0.5

            def max(self, axis=None):  # noqa: ANN001
                if axis == (0, 1):
                    return [0.9, 0.9, 0.9]
                return 0.9

            def min(self, axis=None):  # noqa: ANN001
                if axis == (0, 1):
                    return [0.1, 0.1, 0.1]
                return 0.1

            def __getitem__(self, key: object) -> Arr:
                return self

        saved: list[str] = []

        def save(ext: str) -> bytes:
            saved.append(ext)
            return b"x" * 300

        node = SimpleNamespace(
            family="TOP",
            path="/project1/noise1",
            name="noise1",
            valid=True,
            width=16,
            height=16,
            parent=lambda: None,
            saveByteArray=save,
            numpyArray=lambda delayed=False: Arr(),  # noqa: ARG005
            cook=mock.MagicMock(),
        )
        with mock.patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=node):
            with mock.patch.dict(sys.modules, {"td": SimpleNamespace()}):
                out = tdmcp_bridge.handle_capture({
                    "path": node.path,
                    "mode": "top",
                    "maxSize": None,
                })
        self.assertTrue(out["ok"], out)
        self.assertEqual(saved, [".png"])
        self.assertEqual(out["mimeType"], "image/png")
        self.assertIn("imageBase64", out)
        self.assertNotIn("jpegBase64", out)
        node.cook.assert_not_called()


def _non_uniform_arr():
    class Arr:
        shape = (2, 2, 3)
        ndim = 3
        size = 12

        def mean(self, axis=None):  # noqa: ANN001
            if axis == (0, 1):
                return [0.2, 0.5, 0.8]
            return 0.5

        def max(self, axis=None):  # noqa: ANN001
            if axis == (0, 1):
                return [0.9, 0.9, 0.9]
            return 0.9

        def min(self, axis=None):  # noqa: ANN001
            if axis == (0, 1):
                return [0.1, 0.1, 0.1]
            return 0.1

        def __getitem__(self, key: object) -> Arr:
            return self

    return Arr()


class FakeSharedViewer:
    """Minimal OP Viewer TOP stand-in for shared-viewer capture tests."""

    path = "/project1/tdmcp_rs/capture_viewer"
    width = 8
    height = 8
    family = "TOP"

    def __init__(self) -> None:
        self.par = SimpleNamespace(opviewer=SimpleNamespace(val=None))
        self.cook_calls = 0

    def cook(self, force: bool = False) -> None:  # noqa: ARG002
        self.cook_calls += 1

    def saveByteArray(self, _ext: str) -> bytes:
        return b"x" * 300

    def numpyArray(self, delayed: bool = False):  # noqa: ARG002
        return _non_uniform_arr()


class CaptureSharedViewerTests(unittest.TestCase):
    def setUp(self) -> None:
        tdmcp_bridge.set_bridge_host(SimpleNamespace(path="/project1/tdmcp_rs"))

    def tearDown(self) -> None:
        tdmcp_bridge._bridge_host_path = None  # type: ignore[attr-defined]

    def test_shared_viewer_preview_success(self) -> None:
        viewer = FakeSharedViewer()
        host = SimpleNamespace(
            path="/project1/tdmcp_rs",
            op=lambda name: viewer if name == "capture_viewer" else None,
        )
        source = SimpleNamespace(
            family="SOP",
            path="/project1/zone/sphere1",
            name="sphere1",
            valid=True,
        )
        td_mod = SimpleNamespace(op=lambda p: host if p == "/project1/tdmcp_rs" else None)
        out = tdmcp_bridge._capture_via_shared_viewer(
            td_mod, source, source.path, 256, mode="preview"
        )
        self.assertTrue(out["ok"], out)
        self.assertEqual(out["path"], source.path)
        self.assertEqual(out["mode"], "preview")
        self.assertEqual(out["family"], "SOP")
        self.assertIs(viewer.par.opviewer.val, source)
        self.assertEqual(viewer.cook_calls, 0)
        self.assertEqual(out["mimeType"], "image/png")
        self.assertIn("imageBase64", out)

    def test_shared_viewer_missing_host(self) -> None:
        tdmcp_bridge._bridge_host_path = None  # type: ignore[attr-defined]
        source = SimpleNamespace(family="DAT", path="/project1/table1", name="table1")
        out = tdmcp_bridge._capture_via_shared_viewer(
            SimpleNamespace(op=lambda _p: None),
            source,
            source.path,
            256,
            mode="preview",
        )
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.perception.no_path")

    def test_sequential_preview_retargets(self) -> None:
        """Two back-to-back preview captures must not leak each other's target."""
        viewer = FakeSharedViewer()
        host = SimpleNamespace(
            path="/project1/tdmcp_rs",
            op=lambda name: viewer if name == "capture_viewer" else None,
        )
        td_mod = SimpleNamespace(op=lambda p: host if p == "/project1/tdmcp_rs" else None)
        a = SimpleNamespace(family="MAT", path="/project1/a", name="a", valid=True)
        b = SimpleNamespace(family="DAT", path="/project1/b", name="b", valid=True)
        out_a = tdmcp_bridge._capture_via_shared_viewer(
            td_mod, a, a.path, None, mode="preview"
        )
        self.assertTrue(out_a["ok"], out_a)
        self.assertIs(viewer.par.opviewer.val, a)
        out_b = tdmcp_bridge._capture_via_shared_viewer(
            td_mod, b, b.path, None, mode="preview"
        )
        self.assertTrue(out_b["ok"], out_b)
        self.assertIs(viewer.par.opviewer.val, b)
        self.assertEqual(viewer.cook_calls, 0)

    def test_handle_capture_chop_image_alias(self) -> None:
        source = SimpleNamespace(
            family="CHOP",
            path="/project1/zone/const1",
            name="const1",
            valid=True,
        )
        called: list[str] = []

        def _shared(td_mod, node, path, max_size, *, mode):  # noqa: ANN001, ARG001
            called.append(mode)
            return {
                "ok": True,
                "path": path,
                "mode": mode,
                "family": "CHOP",
                "bytes": 1,
                "mimeType": "image/png",
                "imageBase64": "QQ==",
                "maxSize": max_size,
            }

        with mock.patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=source):
            with mock.patch.object(
                tdmcp_bridge, "_capture_via_shared_viewer", side_effect=_shared
            ):
                with mock.patch.dict(sys.modules, {"td": SimpleNamespace()}):
                    out = tdmcp_bridge.handle_capture({
                        "path": source.path,
                        "mode": "chop_image",
                    })
        self.assertTrue(out["ok"], out)
        self.assertEqual(out["mode"], "chop_image")
        self.assertEqual(called, ["chop_image"])

    def test_handle_capture_pop_alias(self) -> None:
        source = SimpleNamespace(
            family="POP",
            path="/project1/zone/pop1",
            name="pop1",
            valid=True,
        )
        called: list[str] = []

        def _shared(td_mod, node, path, max_size, *, mode):  # noqa: ANN001, ARG001
            called.append(mode)
            return {"ok": True, "path": path, "mode": mode, "family": "POP"}

        with mock.patch.object(tdmcp_bridge, "tdmcp_resolve", return_value=source):
            with mock.patch.object(
                tdmcp_bridge, "_capture_via_shared_viewer", side_effect=_shared
            ):
                with mock.patch.dict(sys.modules, {"td": SimpleNamespace()}):
                    out = tdmcp_bridge.handle_capture({
                        "path": source.path,
                        "mode": "pop",
                    })
        self.assertTrue(out["ok"], out)
        self.assertEqual(called, ["pop"])


if __name__ == "__main__":
    unittest.main()
