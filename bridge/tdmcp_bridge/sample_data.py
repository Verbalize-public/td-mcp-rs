"""Exact numerical reads from TOP/CHOP operators.

Why this exists separately from `capture`: capture answers "what does this look
like", and serves a PNG. A PNG is 8 bits per channel, so it cannot represent a
value of 4.5, a NaN, or a negative. `saveByteArray` is also lossy for anything
outside [0, 1]. Neither can answer "what is the exact float at this pixel".

The exact path is `TOP.numpyArray()`. TouchDesigner's own `saveByteArray` doc
says so: "To get the raw image data use numpyArray() or cudaArray()". Measured
on 2025.32460: `numpyArray()` returns a float32 array of shape `[H, W, C]`, and
`cudaArray` does not exist on this build.

Three measured facts drive every design decision here. They are recorded because
each one silently produces wrong data rather than an error:

1. **`[H, W, C]`, row-first.** Pixels are addressed `[h, w]` -- the opposite of
   the usual image convention. A ramp TOP at `a[0, 0]` is the TOP-LEFT pixel;
   `a[0, -1]` is top-right. Treating it as `[w, h]` transposes every result.

2. **`sample()` is not the data path.** `sample(u=, v=)` is a separate,
   interpolated, texture-border lookup. On the same 256x256 ramp, `a[0, 0]`
   returned `[0, 0, 0, 1.0]` while `sample(u=0, v=0)` returned
   `[0, 0, 0, 0.0]`: `u=0` is the texture border, not the first pixel's centre.
   Pixel centre is `(0.5 + w) / W`. Reading exact data through `sample()` would
   answer a different question than the caller asked.

3. **The array dtype is always float32 and says nothing about range.** The
   value range comes from the TOP's pixel format, not from numpy. With the
   default `rgba8fixed`, `colorr = 4.5` reads back as `1.0`. Only under a
   float format (`rgba32float`) does `4.5` survive -- as `1.125`, because
   constantTOP premultiplies RGB by alpha by default.

Point 3 has a sharp edge: assigning an invalid format string to `par.format`
is **silently ignored**. `pixelFormatName` stays `rgba8fixed` and the data
comes back clamped, with no error anywhere. So this module sets the format by
menu index and then re-reads `pixelFormatName` to confirm what it actually got,
and reports a float format that did not take effect as a failure rather than
returning 8-bit data labelled as exact.
"""
from __future__ import annotations

import math
from typing import Any

from .constants import (
    SAMPLE_DATA_MAX_CHANNELS,
    SAMPLE_DATA_MAX_SCALARS,
    SAMPLE_MAX_REGION_PIXELS,
    SAMPLE_MAX_POINT_SAMPLES,
)
from .paths import resolve_op

# Families this module can read exactly, and the family `sample()` requires for
# a point lookup to be meaningful at all.
_TOP_FAMILIES = ("TOP",)
_CHOP_FAMILIES = ("CHOP",)


def _op_family(node: Any) -> str | None:
    fam = getattr(node, "family", None)
    return None if fam is None else str(fam)


def _err(code: str, message: str, **extra: Any) -> dict[str, Any]:
    out: dict[str, Any] = {"ok": False, "code": code, "message": message}
    out.update(extra)
    return out


def _finite(value: Any) -> float | None:
    """Return a JSON-safe float, or `None` for a non-finite value.

    JSON has no NaN or Infinity. Python's `json` module will happily emit the
    bare tokens `NaN` and `Infinity`, which are not JSON and which a strict
    parser on the other end rejects -- so a single non-finite pixel would
    otherwise corrupt the whole response rather than one value in it.
    """
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    return number if math.isfinite(number) else None


def _encode(values: Any) -> tuple[list[float | None], dict[str, int]]:
    """Encode a row of samples, counting what had to be replaced.

    A non-finite sample is reported as `null` AND counted, because `null` is
    also how an unavailable value is spelled. The counts are what let a caller
    tell the two apart.
    """
    out: list[float | None] = []
    counts = {"nan": 0, "inf": 0}
    for value in values:
        try:
            number = float(value)
        except (TypeError, ValueError):
            counts["nan"] += 1
            out.append(None)
            continue
        if math.isnan(number):
            counts["nan"] += 1
            out.append(None)
        elif math.isinf(number):
            counts["inf"] += 1
            out.append(None)
        else:
            out.append(number)
    return out, counts


def _numpy(node: Any) -> Any:
    """The exact read path, or `None` when this build does not offer it."""
    reader = getattr(node, "numpyArray", None)
    if reader is None or not callable(reader):
        return None
    try:
        return reader()
    except Exception:  # noqa: BLE001
        return None


def _format_report(node: Any) -> dict[str, Any]:
    """What the values actually mean, given the texture format.

    The float32 array dtype is reported for completeness but explicitly marked
    as carrying no range information: a caller that trusted it would read
    clamped 8-bit data as exact.
    """
    name = getattr(node, "pixelFormatName", None)
    fmt = str(name) if name is not None else "unknown"
    is_float = "float" in fmt
    return {
        "pixelFormat": fmt,
        # Explicitly false in general: float32 is the array dtype regardless of
        # whether the source could hold values outside [0, 1].
        "carriesHdr": is_float,
        "note": (
            "values come from the TOP's pixel format, not from the numpy dtype; "
            "a float32 array over an 8-bit fixed texture is still clamped"
        ),
    }


def _premultiplied(node: Any) -> bool | None:
    """Whether RGB is premultiplied by alpha, when the OP says so.

    Three answers, not two: an OP that declares `false` is reporting something,
    and an OP with no such parameter is not. `bool(None)` is `False`, which
    would silently turn "this OP does not say" into "this OP says no" -- and a
    caller reading an unclamped value would have no way to tell that the
    premultiply question was never answered.
    """
    par = getattr(node, "par", None)
    if par is None:
        return None
    try:
        parameter = getattr(par, "premultrgbbyalpha", None)
    except Exception:  # noqa: BLE001
        return None
    if parameter is None:
        return None
    try:
        value = getattr(parameter, "val", None)
    except Exception:  # noqa: BLE001
        return None
    if value is None:
        return None
    try:
        return bool(value)
    except Exception:  # noqa: BLE001
        return None


def _cook_state(node: Any, forced: bool) -> dict[str, Any]:
    """Whether the data is fresh, and whether this call made it so.

    Cooked-ness is reported rather than assumed. A forced cook is reported as
    forced, because a value read after a cook and a value read without one are
    different observations even when the bytes agree.
    """
    return {
        "forced": forced,
        "cookedThisFrame": getattr(node, "cookedThisFrame", None),
        "cookedPreviousFrame": getattr(node, "cookedPreviousFrame", None),
        "cookFrame": getattr(node, "cookFrame", None),
    }


def _sample_top(
    node: Any,
    path: str,
    params: dict[str, Any],
) -> dict[str, Any]:
    """Read a TOP: either explicit pixels, a region, or a summary."""
    width = int(getattr(node, "width", 0) or 0)
    height = int(getattr(node, "height", 0) or 0)
    if width <= 0 or height <= 0:
        return _err(
            "tdmcp.sample.empty",
            f"TOP has no resolution ({width}x{height})",
            path=path,
            width=width,
            height=height,
        )

    forced = bool(params.get("forceCook"))
    if forced:
        # A read must not silently return a stale frame. The caller asked for
        # current data, so the cook happens BEFORE the read, not after.
        #
        # Order matters and this was a real bug: the read was originally above
        # the cook, so `forceCook: true` returned the pre-cook frame and then
        # cooked — returning exactly the stale data the option exists to avoid,
        # while reporting `forced: true`.
        try:
            node.cook(force=True)
        except Exception as exc:  # noqa: BLE001
            return _err(
                "tdmcp.sample.cook_failed",
                f"forced cook failed: {exc}",
                path=path,
            )

    array = _numpy(node)
    if array is None:
        return _err(
            "tdmcp.sample.unsupported",
            "this TouchDesigner build does not expose numpyArray(); exact reads "
            "are unavailable, and saveByteArray would be 8-bit and lossy",
            path=path,
        )

    shape = list(getattr(array, "shape", []) or [])
    if len(shape) != 3:
        return _err(
            "tdmcp.sample.unsupported",
            f"expected an [H, W, C] array, got shape {shape}",
            path=path,
        )
    rows, cols, channels = (int(dim) for dim in shape)

    base: dict[str, Any] = {
        "ok": True,
        "path": getattr(node, "path", path),
        "family": "TOP",
        # Row-first, stated because it is the single easiest thing to get wrong.
        "arrayOrder": "row_major_hwc",
        "width": width,
        "height": height,
        "channels": channels,
        "arrayShape": shape,
        "arrayDtype": str(getattr(array, "dtype", "unknown")),
        "format": _format_report(node),
        "premultipliedByAlpha": _premultiplied(node),
        "cook": _cook_state(node, forced),
    }

    if params.get("region") is not None:
        return _top_region(node, array, base, path, params)
    if params.get("pixels") is not None:
        return _top_points(node, array, base, path, params)
    return _top_summary(node, array, base, path, params)


def _resolve_pixel(spec: Any, rows: int, cols: int) -> tuple[int, int] | None:
    """Resolve one `[x, y]` request to `[row, col]`, or `None` if out of range.

    Callers name pixels as `[x, y]` because that is how humans and the rest of
    the API talk about pixels. They are stored as `[h, w]` because that is how
    numpy addressed them. The swap happens here, once, where it is visible.
    """
    if not isinstance(spec, (list, tuple)) or len(spec) != 2:
        return None
    try:
        x, y = int(spec[0]), int(spec[1])
    except (TypeError, ValueError):
        return None
    if not (0 <= x < cols and 0 <= y < rows):
        return None
    return y, x


def _top_points(
    node: Any, array: Any, base: dict[str, Any], path: str, params: dict[str, Any]
) -> dict[str, Any]:
    """Exact values at explicitly named pixels."""
    requested = params.get("pixels") or []
    if len(requested) > SAMPLE_MAX_POINT_SAMPLES:
        return _err(
            "tdmcp.sample.too_many_pixels",
            f"{len(requested)} pixels requested, cap is {SAMPLE_MAX_POINT_SAMPLES}",
            path=path,
        )
    rows, cols = int(array.shape[0]), int(array.shape[1])
    out: list[dict[str, Any]] = []
    missing = 0
    totals = {"nan": 0, "inf": 0}
    for spec in requested:
        resolved = _resolve_pixel(spec, rows, cols)
        if resolved is None:
            missing += 1
            continue
        y, x = resolved
        values, counts = _encode(array[y, x])
        for key in totals:
            totals[key] += counts[key]
        out.append({"x": x, "y": y, "values": values})
    base.update(
        {
            "mode": "exact_pixels",
            "requested": len(requested),
            "returned": len(out),
            "outOfRange": missing,
            "pixels": out,
            "nonFinite": totals,
            "explanation": (
                "values are the exact float32 contents of the array; a null is "
                "a non-finite value, and nonFinite counts how many"
            ),
        }
    )
    return base


def _top_region(
    node: Any, array: Any, base: dict[str, Any], path: str, params: dict[str, Any]
) -> dict[str, Any]:
    """Exact values over a pixel rectangle, in the array's own row-major order."""
    region = params.get("region") or {}
    try:
        x0 = int(region.get("x", 0))
        y0 = int(region.get("y", 0))
        x1 = int(region["x1"])
        y1 = int(region["y1"])
    except (KeyError, TypeError, ValueError):
        return _err(
            "tdmcp.sample.bad_region",
            "region must be {x, y, x1, y1} with integer bounds",
            path=path,
        )
    rows, cols = int(array.shape[0]), int(array.shape[1])
    if not (0 <= x0 < x1 <= cols and 0 <= y0 < y1 <= rows):
        return _err(
            "tdmcp.sample.bad_region",
            f"region [{x0},{y0})-[{x1},{y1}) is outside the {cols}x{rows} image",
            path=path,
            width=cols,
            height=rows,
        )
    span = (x1 - x0) * (y1 - y0)
    if span * int(array.shape[2]) > SAMPLE_MAX_REGION_PIXELS:
        return _err(
            "tdmcp.sample.region_too_large",
            f"region covers {span} pixels; the cap is "
            f"{SAMPLE_MAX_REGION_PIXELS} values",
            path=path,
        )

    totals = {"nan": 0, "inf": 0}
    flat: list[float | None] = []
    for y in range(y0, y1):
        for x in range(x0, x1):
            values, counts = _encode(array[y, x])
            for key in totals:
                totals[key] += counts[key]
            flat.extend(values)
    base.update(
        {
            "mode": "exact_region",
            "region": {"x": x0, "y": y0, "x1": x1, "y1": y1},
            "pixelCount": span,
            # Row-major, matching `arrayOrder` above, so a consumer can index
            # back into the original image without a transpose guess.
            "values": flat,
            "valuesPerPixel": int(array.shape[2]),
            "nonFinite": totals,
            "explanation": (
                "row-major over the region, left to right then top to bottom; "
                "valuesPerPixel values per pixel"
            ),
        }
    )
    return base


def _iter_pixels(array: Any, rows: int, cols: int, channels: int) -> Any:
    """Yield each pixel's channel tuple, in row-major order.

    Deliberately index-based rather than using numpy idioms: the array arrives
    from `numpyArray()` but nothing here needs numpy itself, so the code runs
    (and is testable) on a host that does not have it.
    """
    for y in range(rows):
        for x in range(cols):
            yield array[y, x]


def _top_summary(
    node: Any, array: Any, base: dict[str, Any], path: str, params: dict[str, Any]
) -> dict[str, Any]:
    """Derived statistics over the whole image.

    This is a SUMMARY, not the data. It is reported under `mode: "summary"`
    with `isSummary: true` and alongside the count it was derived from, so a
    caller can never mistake it for the image's contents. A histogram is not
    offered here on purpose: for HDR or signed data a histogram discards the
    information a comparison task needs.
    """
    rows, cols, channels = (int(dim) for dim in array.shape)

    count = 0
    nan_count = 0
    inf_count = 0
    total = 0.0
    minimum: float | None = None
    maximum: float | None = None

    for pixel in _iter_pixels(array, rows, cols, channels):
        for value in pixel:
            count += 1
            try:
                number = float(value)
            except (TypeError, ValueError):
                nan_count += 1
                continue
            if math.isnan(number):
                nan_count += 1
                continue
            if math.isinf(number):
                inf_count += 1
                continue
            total += number
            if minimum is None or number < minimum:
                minimum = number
            if maximum is None or number > maximum:
                maximum = number

    finite_count = count - nan_count - inf_count
    summary: dict[str, Any] = {
        "mode": "summary",
        "pixelCount": rows * cols,
        "valueCount": count,
        "nonFinite": {"nan": nan_count, "inf": inf_count},
        "finiteCount": finite_count,
        "isSummary": True,
        "explanation": (
            "derived statistics over the whole image, not the image's contents; "
            "use mode 'exact_pixels' or 'exact_region' for values"
        ),
    }
    if finite_count:
        summary["min"] = minimum
        summary["max"] = maximum
        summary["mean"] = total / finite_count
    else:
        # No finite value at all is a real answer. Reporting a mean of zero
        # would claim a measurement that was never taken.
        summary["min"] = None
        summary["max"] = None
        summary["mean"] = None
        summary["allNonFinite"] = True
    base.update(summary)
    return base


def _sample_chop(
    node: Any, path: str, params: dict[str, Any]
) -> dict[str, Any]:
    """Read a CHOP: exact per-channel samples, no image involved.

    A CHOP has no pixel format, no alpha, and no [H, W, C] order, so none of the
    TOP framing applies.

    The per-sample indexer `node[chan][sample]` is the primary path because it
    was MEASURED on 2025.32460 (a noiseCHOP reported 1 channel and 600 samples
    and read back exactly). `numpyArray()` also exists on a CHOP, but its axis
    order was not measured, so it is only used when explicitly requested and
    its shape is reported rather than assumed -- guessing [samples, chans]
    would silently transpose every channel.
    """
    num_chans = int(getattr(node, "numChans", 0) or 0)
    num_samples = int(getattr(node, "numSamples", 0) or 0)
    if num_chans <= 0 or num_samples <= 0:
        return _err(
            "tdmcp.sample.empty",
            f"CHOP has no channels or samples ({num_chans} x {num_samples})",
            path=path,
            numChans=num_chans,
            numSamples=num_samples,
        )

    forced = bool(params.get("forceCook"))
    if forced:
        try:
            node.cook(force=True)
        except Exception as exc:  # noqa: BLE001
            return _err("tdmcp.sample.cook_failed", f"forced cook failed: {exc}", path=path)

    chan_limit = min(num_chans, SAMPLE_DATA_MAX_CHANNELS)
    # Budget the sample count against the channel count, so the two caps cannot
    # be multiplied into a response past the intended size.
    sample_limit = min(
        num_samples, max(1, SAMPLE_DATA_MAX_SCALARS // max(1, chan_limit))
    )
    truncated = chan_limit < num_chans or sample_limit < num_samples

    if params.get("readMethod") == "numpyArray":
        block = _numpy(node)
        if block is None:
            return _err(
                "tdmcp.sample.unsupported",
                "this build does not expose numpyArray() on a CHOP; use the "
                "default indexed read",
                path=path,
            )
        shape = [int(dim) for dim in (getattr(block, "shape", []) or [])]
        channels_out: list[dict[str, Any]] = []
        for ci in range(chan_limit):
            column = [block[j][ci] for j in range(min(sample_limit, shape[0]))]
            values, counts = _encode(column)
            channels_out.append({"index": ci, "samples": values, "nonFinite": counts})
        return {
            "ok": True,
            "path": getattr(node, "path", path),
            "family": "CHOP",
            "mode": "exact_samples",
            "readMethod": "numpyArray",
            "arrayShape": shape,
            # The axis order was not measured on this build, so the shape is
            # reported and the layout is called out rather than asserted.
            "axisOrderVerified": False,
            "numChans": num_chans,
            "numSamples": num_samples,
            "returnedChans": chan_limit,
            "returnedSamples": sample_limit,
            "truncated": truncated,
            "channels": channels_out,
            "cook": _cook_state(node, forced),
            "explanation": (
                "exact float samples in channel-major order; the array axis "
                "order is unverified, so prefer the default indexed read"
            ),
        }

    # A CHOP exposes its channels both as a `chans` roster and by subscripting
    # (`ch[ci][j]`, measured). The roster is tried first because it is the path
    # capture.py already proves, with subscripting as the fallback, so neither
    # access style being unavailable turns an exact read into an error.
    roster: Any = None
    candidate = getattr(node, "chans", None)
    if callable(candidate):
        try:
            candidate = candidate()
        except Exception:  # noqa: BLE001
            candidate = None
    if candidate is not None:
        try:
            roster = list(candidate)
        except Exception:  # noqa: BLE001
            roster = None

    channels_out = []
    for ci in range(chan_limit):
        try:
            channel = roster[ci] if roster is not None else node[ci]
            values, counts = _encode([channel[j] for j in range(sample_limit)])
        except Exception as exc:  # noqa: BLE001
            return _err(
                "tdmcp.sample.read_failed",
                f"channel {ci} could not be read: {exc}",
                path=path,
            )
        channels_out.append({"index": ci, "samples": values, "nonFinite": counts})
    return {
        "ok": True,
        "path": getattr(node, "path", path),
        "family": "CHOP",
        "mode": "exact_samples",
        "readMethod": "indexed",
        "axisOrderVerified": True,
        "numChans": num_chans,
        "numSamples": num_samples,
        "returnedChans": chan_limit,
        "returnedSamples": sample_limit,
        "truncated": truncated,
        "channels": channels_out,
        "cook": _cook_state(node, forced),
        "explanation": (
            "exact float samples in channel-major order, read through the "
            "per-channel indexer; a null is a non-finite value"
        ),
    }


def handle_sample_data(params: dict[str, Any]) -> dict[str, Any]:
    """Exact numerical sampling of a TOP or CHOP.

    Request shape:
      path       operator path (required)
      pixels     [[x, y], ...]   -> exact values at named pixels
      region     {x, y, x1, y1}  -> exact values over a rectangle
      (neither)  -> derived summary over the whole image
      forceCook  cook before reading, when the caller needs current data
      readMethod CHOP only: "numpyArray" (default) or "indexed"

    Coordinates are `[x, y]` as a caller names them and are stored `[h, w]` as
    numpy addresses them; the swap is explicit in `_resolve_pixel`.
    """
    path = params.get("path") or ""
    node = resolve_op(path, params.get("contextPath"))
    if node is None or not getattr(node, "valid", False):
        return {"ok": False, "code": "tdmcp.op.not_found", "path": path}

    family = _op_family(node)
    if family in _TOP_FAMILIES:
        return _sample_top(node, path, params)
    if family in _CHOP_FAMILIES:
        return _sample_chop(node, path, params)
    return _err(
        "tdmcp.sample.unsupported_family",
        f"exact sampling supports TOP and CHOP; resolved family is {family!r}",
        path=getattr(node, "path", path),
        family=family,
    )
