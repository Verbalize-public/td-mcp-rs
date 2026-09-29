"""The artifact identity + transformation record on every encoded capture.

C26 requires that a preview never masquerade as the original: an exact
pixel/numeric task must be able to see that what it received was resampled.
Before this record existed the response carried only `maxSize` — what was
REQUESTED — so a 512px downscale of a 4K operator was indistinguishable
from a native 512px operator. `maxSize: 512` on a 512px TOP and
`maxSize: 512` on a 4096px TOP produced byte-identical metadata.

These tests assert the transformation flag against the numbers printed
beside it, so the flag cannot drift from the dimensions it describes, and
each asserts its PRECONDITION first (that the downscale really happened) so
it cannot degrade into a test that cannot fail.
"""

from __future__ import annotations

import os
import sys
from types import SimpleNamespace

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402


class _Param:
    """One TD parameter: `.val` is the value."""

    def __init__(self, val: object = 0) -> None:
        self.val = val

    @property
    def v(self) -> object:
        return self.val


class _Par:
    """A TD `par` stand-in.

    The real `par` routes `top.par.resolutionw = 512` into that parameter's
    VALUE. A plain namespace would instead REBIND the name, so reading it
    back would find an int with no `.val` -- the double would then be
    measuring the product's reaction to a broken test rather than to a real
    capture, and the artifact record would come back 0x0 and
    `dimensions: unavailable`.
    """

    def __init__(self) -> None:
        object.__setattr__(self, "_params", {})

    def _param(self, name: str) -> _Param:
        params = object.__getattribute__(self, "_params")
        if name not in params:
            params[name] = _Param(0)
        return params[name]

    def __getattr__(self, name: str) -> _Param:
        if name.startswith("_"):
            raise AttributeError(name)
        return self._param(name)

    def __setattr__(self, name: str, value: object) -> None:
        self._param(name).val = value


class FakeResolutionTOP:
    """A resolutionTOP that reports the resolution it was configured with.

    A plain class, not a `SimpleNamespace`: the latter implements
    `__setattr__` in C, so a Python override is never reached and `width`
    would be set to a plain attribute rather than derived from the par.
    """

    def __init__(self, name: str, payload: bytes = b"") -> None:
        self.name = name
        self.par = _Par()
        self.inputConnectors = [SimpleNamespace(connect=lambda _src: None)]
        # The temporary is what actually gets encoded, so it needs the
        # capture surface too. Encoding from the ORIGINAL instead would let
        # a test pass while the product encoded the resample.
        self.saveByteArray = lambda *_a, **_kw: payload

    @property
    def width(self) -> int:
        return int(self.par.resolutionw.val or 0)

    @property
    def height(self) -> int:
        return int(self.par.resolutionh.val or 0)

    def destroy(self) -> None:
        self.destroyed = True


def _fake_td_module(payload: bytes = b"") -> SimpleNamespace:
    return SimpleNamespace(resolutionTOP=lambda name: FakeResolutionTOP(name, payload))


def _top(width: int, height: int, payload: bytes, path: str = "/project1/out1"):
    """A TOP that encodes `payload` and reports the given resolution."""

    def _save(*_a: object, **_kw: object) -> bytes:
        return payload

    return SimpleNamespace(
        path=path,
        name="out1",
        width=width,
        height=height,
        saveByteArray=_save,
        parent=lambda: SimpleNamespace(
            op=lambda _n: None,
            create=lambda _cls, name: FakeResolutionTOP(name, payload),
        ),
    )


def test_a_native_capture_states_that_no_transformation_happened() -> None:
    node = _top(512, 256, b"png-bytes")
    out = tdmcp_bridge._capture_top_image(None, node, node.path, None)

    assert out["ok"] is True
    art = out["artifact"]
    # Precondition first: these must be the real dimensions, not zeros.
    assert art["source"] == {"width": 512, "height": 256}, art
    assert art["delivered"] == {"width": 512, "height": 256}, art
    assert art["transformed"] is False, art
    assert art["transformation"] == "none", art
    assert art["encodedBytes"] == len(b"png-bytes"), art


def test_a_downscale_is_reported_rather_than_looking_native() -> None:
    node = _top(4096, 2048, b"png-bytes")
    out = tdmcp_bridge._capture_top_image(_fake_td_module(b"x"), node, node.path, 512)

    assert out["ok"] is True
    art = out["artifact"]
    # Precondition: the downscale really ran, so this is not asserting a
    # flag about a path that never executed.
    assert art["source"] == {"width": 4096, "height": 2048}, art
    assert art["delivered"] == {"width": 512, "height": 256}, art
    assert art["transformed"] is True, (
        "a resampled capture reported no transformation: the caller cannot "
        "tell a preview from the operator's own pixels"
    )
    assert art["transformation"] == "downscale", art


def test_a_requested_size_already_at_the_source_resolution_is_not_a_transformation() -> None:
    """`maxSize` equal to the source is a NO-OP and must not claim otherwise.

    The flag is derived from the dimensions, not from whether a temporary
    node was created. `_maybe_downscale_top` returns the target untouched
    when `longest <= max_size`, so a real call on a 512px TOP with
    `maxSize: 512` builds nothing — and must say so.
    """
    node = _top(512, 512, b"png-bytes")
    out = tdmcp_bridge._capture_top_image(_fake_td_module(b"x"), node, node.path, 512)

    art = out["artifact"]
    assert art["source"] == {"width": 512, "height": 512}, art
    assert art["delivered"] == {"width": 512, "height": 512}, art
    assert art["transformed"] is False, (
        "an unchanged capture claimed a transformation: this would make every "
        "capture look lossy and train callers to ignore the field"
    )
    assert art["transformation"] == "none", art


def test_the_flag_always_agrees_with_the_numbers_beside_it() -> None:
    """`transformed` is DERIVED, so the two cannot drift.

    Every combination of (native, exact-size, downscaled) must satisfy
    `transformed == (source != delivered)`. A flag computed from "did we
    build a temporary node" would be True whenever a node was built, which
    is a different claim.
    """
    cases = [
        (512, 256, None),
        (512, 512, 512),
        (4096, 2048, 512),
        (1024, 4096, 512),
        (300, 200, 1000),
    ]
    for width, height, max_size in cases:
        node = _top(width, height, b"x")
        out = tdmcp_bridge._capture_top_image(_fake_td_module(b"x"), node, node.path, max_size)
        art = out["artifact"]
        source = (art["source"]["width"], art["source"]["height"])
        delivered = (art["delivered"]["width"], art["delivered"]["height"])
        assert art["transformed"] is (source != delivered), (
            f"flag disagrees with dimensions for {width}x{height} @ {max_size}: {art}"
        )
        assert art["transformation"] == ("downscale" if source != delivered else "none"), art


def test_the_temporary_operator_is_still_destroyed_after_a_downscale() -> None:
    """The record must not cost the caller their scene.

    `_maybe_downscale_top` creates a resolutionTOP under the operator's own
    parent. A `finally` that stopped destroying it would leave one node per
    capture in the user's project — and the artifact record would be the
    first place a caller could see it happen.
    """
    created: list[FakeResolutionTOP] = []

    def _create(_cls: object, name: str) -> FakeResolutionTOP:
        top = FakeResolutionTOP(name)
        created.append(top)
        return top

    node = _top(4096, 2048, b"x")
    node.parent = lambda: SimpleNamespace(op=lambda _n: None, create=_create)

    tdmcp_bridge._capture_top_image(_fake_td_module(b"x"), node, node.path, 512)

    assert len(created) == 1, f"expected one temporary, got {len(created)}"
    assert getattr(created[0], "destroyed", False) is True, (
        "the temporary resolutionTOP was left in the operator's parent"
    )


def test_a_refused_capture_still_reports_what_it_refused() -> None:
    """A rejection must not carry an artifact block.

    There are no pixels, so there is no artifact. Emitting a zeroed block
    would be a null-shaped lie — `0x0` reads as "this operator has no
    resolution" rather than "nothing was captured".
    """
    node = _top(4096, 4096, b"x")
    out = tdmcp_bridge._capture_top_image(None, node, node.path, None)

    assert out["ok"] is False
    assert out["code"] == "tdmcp.perception.max_size_too_large"
    assert "artifact" not in out, out


def test_unknown_dimensions_are_reported_unavailable_not_zero() -> None:
    """An operator that will not report its resolution says so.

    A 0x0 block would be indistinguishable from a real zero-size capture,
    and `transformed: (0,0) != (0,0)` would read as "native".
    """

    class Silent(SimpleNamespace):
        pass

    node = Silent(path="/project1/out1", saveByteArray=lambda *_a, **_kw: b"x")
    # No width/height attributes at all — `getattr(..., 0) or 0` yields 0.
    out = tdmcp_bridge._capture_top_image(None, node, node.path, None)

    art = out["artifact"]
    assert art["dimensions"] == "unavailable", art
    assert art["transformed"] is False, art
    assert art["transformation"] == "none", art


def test_encoded_bytes_counts_the_png_not_the_base64() -> None:
    """`encodedBytes` is the artifact's size, not its transport's.

    Base64 inflates by 4/3; a record that reported the wire size would
    describe a value the caller does not have.
    """
    payload = bytes(range(256)) * 4
    node = _top(512, 512, payload)
    out = tdmcp_bridge._capture_top_image(None, node, node.path, None)

    assert out["bytes"] == len(payload)
    assert out["artifact"]["encodedBytes"] == len(payload)
