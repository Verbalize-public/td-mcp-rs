"""Unit tests for capture's hard `maxSize` pre-flight cap (no live TD)."""

from __future__ import annotations

import os
import sys
from types import SimpleNamespace

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402


def _explode(*_a: object, **_kw: object) -> None:
    raise AssertionError("saveByteArray must not run once the cap rejects")


def _fake_top(
    *, width: int, height: int, path: str = "/project1/zone/big", capture_ok: bool = False
) -> SimpleNamespace:
    return SimpleNamespace(
        path=path,
        width=width,
        height=height,
        saveByteArray=(lambda *_a, **_kw: b"") if capture_ok else _explode,
    )


def test_explicit_max_size_above_cap_is_rejected_before_capture() -> None:
    node = _fake_top(width=64, height=64)
    out = tdmcp_bridge._capture_top_image(
        None, node, node.path, tdmcp_bridge.CAPTURE_MAX_SIZE + 1
    )
    assert out["ok"] is False
    assert out["code"] == "tdmcp.perception.max_size_too_large"


def test_explicit_max_size_at_cap_passes_the_gate() -> None:
    node = _fake_top(width=64, height=64, capture_ok=True)
    out = tdmcp_bridge._capture_top_image(None, node, node.path, tdmcp_bridge.CAPTURE_MAX_SIZE)
    assert out.get("code") != "tdmcp.perception.max_size_too_large"


def test_native_resolution_over_cap_is_rejected() -> None:
    node = _fake_top(width=4096, height=4096)
    out = tdmcp_bridge._capture_top_image(None, node, node.path, None)
    assert out["ok"] is False
    assert out["code"] == "tdmcp.perception.max_size_too_large"
    assert "4096" in out["message"]


def test_native_resolution_under_cap_passes_the_gate() -> None:
    node = _fake_top(width=64, height=64, capture_ok=True)
    out = tdmcp_bridge._capture_top_image(None, node, node.path, None)
    assert out.get("code") != "tdmcp.perception.max_size_too_large"
