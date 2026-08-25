"""Perception capture (TOP / CHOP / shared OP Viewer)."""
from __future__ import annotations

from typing import Any

from . import state as _state
from .constants import (
    CAPTURE_MAX_SIZE,
    CAPTURE_VIEWER_NAME,
    CHOP_DATA_MAX_CHANNELS,
    CHOP_DATA_MAX_SAMPLES,
    CHOP_DATA_MAX_SCALARS,
    _BLACK_MEAN_THRESHOLD,
    _UNIFORM_RANGE_THRESHOLD,
)
from .paths import resolve_op

def _op_family(node: Any) -> str | None:
    """Best-effort TD family string (``TOP`` / ``CHOP`` / ``COMP`` / ``POP`` / …)."""
    fam = getattr(node, "family", None)
    if fam is None:
        return None
    return str(fam)


def _effective_capture_mode(mode: str, node: Any) -> str:
    """Resolve ``auto`` (and pass-through explicit modes) from operator family.

    ``TOP`` → ``top``; ``CHOP`` → ``chop_data`` (numeric default); everything
    else (COMP/POP/SOP/MAT/DAT/unrecognized) → ``preview`` via shared OP Viewer.
    """
    if mode != "auto":
        return mode
    family = _op_family(node)
    if family == "CHOP":
        return "chop_data"
    if family == "TOP" or hasattr(node, "saveByteArray"):
        return "top"
    return "preview"


def _chan_samples(chan: Any, n_samples: int) -> list[float]:
    """Read up to ``n_samples`` floats from a TD Channel (or test double)."""
    vals = getattr(chan, "vals", None)
    if vals is not None:
        try:
            return [float(vals[i]) for i in range(n_samples)]
        except Exception:  # noqa: BLE001
            pass
    out: list[float] = []
    for i in range(n_samples):
        try:
            out.append(float(chan[i]))
        except Exception:  # noqa: BLE001
            out.append(0.0)
    return out


def _capture_chop_data(node: Any, path: str) -> dict[str, Any]:
    """CHOP → capped JSON. Pure enough for unit tests with fake CHOPs."""
    resolved = getattr(node, "path", None) or path
    family = _op_family(node) or "CHOP"
    if family != "CHOP":
        return {
            "ok": False,
            "code": "tdmcp.perception.wrong_family",
            "message": (
                f"mode chop_data requires CHOP; resolved family is {family!r}"
            ),
            "path": resolved,
            "mode": "chop_data",
            "family": family,
        }

    num_chans = int(getattr(node, "numChans", 0) or 0)
    num_samples = int(getattr(node, "numSamples", 0) or 0)
    if num_chans <= 0 or num_samples <= 0:
        return {
            "ok": False,
            "code": "tdmcp.perception.empty_chop",
            "message": (
                f"CHOP has no channels or samples "
                f"(numChans={num_chans}, numSamples={num_samples})"
            ),
            "path": resolved,
            "mode": "chop_data",
            "family": "CHOP",
            "numChans": num_chans,
            "numSamples": num_samples,
        }

    chans_attr = getattr(node, "chans", None)
    if callable(chans_attr):
        try:
            chans_attr = chans_attr()
        except Exception:  # noqa: BLE001
            chans_attr = None
    if chans_attr is None:
        chans_list: list[Any] = []
        for i in range(num_chans):
            try:
                chans_list.append(node[i])
            except Exception:  # noqa: BLE001
                break
    else:
        chans_list = list(chans_attr)

    channels_out: list[dict[str, Any]] = []
    scalars_used = 0
    truncated_field: str | None = None
    truncated_limit = 0
    chan_limit = min(num_chans, CHOP_DATA_MAX_CHANNELS, len(chans_list))

    for ci in range(chan_limit):
        if scalars_used >= CHOP_DATA_MAX_SCALARS:
            truncated_field = "scalars"
            truncated_limit = CHOP_DATA_MAX_SCALARS
            break
        chan = chans_list[ci]
        samples_wanted = min(
            num_samples,
            CHOP_DATA_MAX_SAMPLES,
            CHOP_DATA_MAX_SCALARS - scalars_used,
        )
        samples = _chan_samples(chan, samples_wanted)
        name = getattr(chan, "name", None)
        if name is None:
            name = f"chan{ci}"
        channels_out.append({"name": str(name), "samples": samples})
        scalars_used += len(samples)
        if samples_wanted < num_samples:
            truncated_field = "samples"
            truncated_limit = CHOP_DATA_MAX_SAMPLES
            # Continue other channels only if scalar budget remains; if we
            # hit samples-per-channel cap but still have budget, mark samples
            # and keep going until channel/scalar caps.
            if scalars_used >= CHOP_DATA_MAX_SCALARS:
                truncated_field = "scalars"
                truncated_limit = CHOP_DATA_MAX_SCALARS
                break

    if truncated_field is None and (
        chan_limit < num_chans or len(chans_list) < num_chans
    ):
        truncated_field = "channels"
        truncated_limit = CHOP_DATA_MAX_CHANNELS

    out: dict[str, Any] = {
        "ok": True,
        "path": resolved,
        "mode": "chop_data",
        "family": "CHOP",
        "numChans": num_chans,
        "numSamples": num_samples,
        "channels": channels_out,
    }
    rate = getattr(node, "rate", None)
    if rate is not None:
        try:
            out["rate"] = float(rate)
        except (TypeError, ValueError):
            pass
    if truncated_field is not None:
        out["truncation"] = {
            "field": truncated_field,
            "limit": truncated_limit,
            "code": "tdmcp.perception.chop_truncated",
            "message": (
                f"CHOP capture capped at {truncated_field} "
                f"(limit={truncated_limit}; "
                f"numChans={num_chans}, numSamples={num_samples})"
            ),
            "mitigation": [
                "Narrow the CHOP window or channel count in TD before re-capture",
                "Caps are fixed: 32 channels, 256 samples/channel, 4096 scalars",
            ],
        }
    return out


def _capture_top_image(
    td_mod: Any, target: Any, path: str, max_size: Any
) -> dict[str, Any]:
    """TOP → PNG (+ black/uniform soft-fail). Temps always destroyed."""
    import base64

    if not hasattr(target, "saveByteArray"):
        return {
            "ok": False,
            "code": "tdmcp.perception.no_path",
            "message": "no TOP saveByteArray on resolved path",
            "path": getattr(target, "path", path),
        }

    # Hard pre-flight reject, mirroring SCRIPT_MAX_BYTES: a PNG+base64 payload
    # at unbounded native resolution can blow the 16 MiB IPC frame and kill
    # the whole bridge session, not just this call (docs/LIMITS_AUDIT.md §4.2).
    if max_size is not None:
        if int(max_size) > CAPTURE_MAX_SIZE:
            return {
                "ok": False,
                "code": "tdmcp.perception.max_size_too_large",
                "message": f"maxSize {int(max_size)} exceeds the {CAPTURE_MAX_SIZE}px cap",
                "path": getattr(target, "path", path),
            }
    else:
        width = int(getattr(target, "width", 0) or 0)
        height = int(getattr(target, "height", 0) or 0)
        if max(width, height) > CAPTURE_MAX_SIZE:
            return {
                "ok": False,
                "code": "tdmcp.perception.max_size_too_large",
                "message": (
                    f"native resolution {width}x{height} exceeds the "
                    f"{CAPTURE_MAX_SIZE}px cap; pass an explicit maxSize "
                    f"(<= {CAPTURE_MAX_SIZE}) instead of native"
                ),
                "path": getattr(target, "path", path),
            }

    tmp_top = None
    source = target
    try:
        if max_size is not None:
            source, tmp_top = _maybe_downscale_top(td_mod, target, int(max_size))
        data = source.saveByteArray(".png")
        raw = bytes(data) if data is not None else b""
        kind, mean_rgb = _classify_frame(source, raw)
        code = None
        message = None
        if kind == "black":
            code = "tdmcp.perception.black_frame"
            message = _perception_frame_message("black", mean_rgb)
        elif kind == "uniform":
            code = "tdmcp.perception.uniform_frame"
            message = _perception_frame_message("uniform", mean_rgb)
        return {
            "ok": kind is None,
            "code": code,
            "message": message,
            "bytes": len(raw),
            "path": getattr(target, "path", path),
            "mimeType": "image/png",
            "imageBase64": base64.b64encode(raw).decode("ascii") if raw else None,
            "maxSize": max_size,
        }
    except Exception as exc:  # noqa: BLE001
        return {"ok": False, "error": str(exc), "traceback": traceback.format_exc()}
    finally:
        if tmp_top is not None:
            try:
                tmp_top.destroy()
            except Exception:  # noqa: BLE001
                pass


def _bridge_host(td_mod: Any) -> Any:
    """Resolve the bootstrap COMP that owns ``capture_viewer`` / ``debug``."""
    host_path = _state.get_bridge_host_path()
    if not host_path:
        return None
    try:
        return td_mod.op(host_path)
    except Exception:  # noqa: BLE001
        return None


def _ensure_capture_viewer(td_mod: Any) -> Any:
    """Return the shared OP Viewer TOP under the bridge host (create if missing)."""
    host = _bridge_host(td_mod)
    if host is None:
        return None
    existing = host.op(CAPTURE_VIEWER_NAME) if hasattr(host, "op") else None
    if existing is not None:
        return existing
    op_class = getattr(td_mod, "opviewerTOP", None)
    if op_class is None or not hasattr(host, "create"):
        return None
    try:
        viewer = host.create(op_class, CAPTURE_VIEWER_NAME)
        try:
            viewer.nodeX, viewer.nodeY = 400, 0
        except Exception:  # noqa: BLE001
            pass
        return viewer
    except Exception:  # noqa: BLE001
        return None


def _capture_via_shared_viewer(
    td_mod: Any,
    source: Any,
    path: str,
    max_size: Any,
    *,
    mode: str,
) -> dict[str, Any]:
    """Retarget bridge ``capture_viewer`` at ``source`` → PNG (any family).

    Safe under the per-pid FIFO: only one bridge dispatch runs at a time, so
    ``par.opviewer`` retarget + save is not raced by concurrent capture.
    """
    family = _op_family(source)
    viewer = _ensure_capture_viewer(td_mod)
    if viewer is None:
        return {
            "ok": False,
            "code": "tdmcp.perception.no_path",
            "message": (
                "shared capture_viewer missing "
                "(bridge host not registered or opviewerTOP unavailable)"
            ),
            "path": getattr(source, "path", path),
            "mode": mode,
            "family": family,
        }

    try:
        par = getattr(getattr(viewer, "par", None), "opviewer", None)
        if par is None:
            raise RuntimeError("capture_viewer has no par.opviewer")
        # Prefer source-native resolution (avoid letterboxed black frames).
        out_res = getattr(getattr(viewer, "par", None), "outputresolution", None)
        if out_res is not None:
            try:
                out_res.val = "useinput"
            except Exception:  # noqa: BLE001
                pass
        # Node Viewer must be enabled for OP Viewer TOP to rasterize content.
        try:
            source.viewer = True
        except Exception:  # noqa: BLE001
            pass
        par.val = source
    except Exception as exc:  # noqa: BLE001
        return {
            "ok": False,
            "code": "tdmcp.perception.no_path",
            "message": f"failed to bind capture_viewer: {exc}",
            "path": getattr(source, "path", path),
            "mode": mode,
            "family": family,
            "traceback": traceback.format_exc(),
        }

    result = _capture_top_image(td_mod, viewer, path, max_size)
    # Report the source path agents asked for, not the shared viewer.
    result["path"] = getattr(source, "path", path)
    result["mode"] = mode
    if family is not None:
        result["family"] = family
    if result.get("ok") is False and "error" in result and "code" not in result:
        return {
            "ok": False,
            "code": "tdmcp.perception.no_path",
            "message": str(result.get("error") or "shared viewer capture failed"),
            "path": getattr(source, "path", path),
            "mode": mode,
            "family": family,
            "traceback": result.get("traceback"),
        }
    return result


def handle_capture(params: dict[str, Any]) -> dict[str, Any]:
    """Perception capture: top / preview / auto / chop_data / chop_image / pop.

    Image modes return ``imageBase64`` (PNG) for MCP image promotion.
    ``chop_data`` returns capped channel JSON (no image). Optional ``maxSize``
    (default 256) applies to image paths only via a temp ``resolutionTOP`` that
    is always destroyed. ``preview`` (and aliases ``chop_image`` / ``pop``)
    retarget the bridge's shared ``capture_viewer`` OP Viewer TOP — any family.
    Cooking is left to TD on read / ``saveByteArray`` (no force-cook).
    """
    path = params.get("path") or ""
    mode = params.get("mode") or "auto"
    context_path = params.get("contextPath")
    max_size = params.get("maxSize", 512)
    node = resolve_op(path, context_path)
    if node is None or not getattr(node, "valid", False):
        return {"ok": False, "code": "tdmcp.op.not_found", "path": path}

    effective = _effective_capture_mode(str(mode), node)

    if effective == "chop_data":
        return _capture_chop_data(node, path)

    import td  # type: ignore  # PNG / shared-viewer paths need the TD module

    # chop_image / pop are aliases of preview (shared OP Viewer path).
    if effective in ("preview", "chop_image", "pop"):
        # Call via package so tests can monkeypatch ``tdmcp_bridge._capture_via_shared_viewer``.
        import sys

        pkg = sys.modules.get(__package__)
        shared = (
            getattr(pkg, "_capture_via_shared_viewer", None) if pkg is not None else None
        )
        if not callable(shared):
            shared = _capture_via_shared_viewer
        return shared(td, node, path, max_size, mode=effective)

    if effective == "top":
        # Explicit top on non-TOP → wrong_family (not no_path).
        if _op_family(node) not in (None, "TOP") and not hasattr(
            node, "saveByteArray"
        ):
            fam = _op_family(node)
            return {
                "ok": False,
                "code": "tdmcp.perception.wrong_family",
                "message": f"mode top requires TOP; resolved family is {fam!r}",
                "path": getattr(node, "path", path),
                "mode": "top",
                "family": fam,
            }
        result = _capture_top_image(td, node, path, max_size)
        if (
            result.get("code") == "tdmcp.perception.no_path"
            and _op_family(node) not in (None, "TOP")
        ):
            fam = _op_family(node)
            return {
                "ok": False,
                "code": "tdmcp.perception.wrong_family",
                "message": f"mode top requires TOP; resolved family is {fam!r}",
                "path": getattr(node, "path", path),
                "mode": "top",
                "family": fam,
            }
        return result

    # Unknown mode string (should not happen for schema-validated callers).
    fam = _op_family(node)
    return {
        "ok": False,
        "code": "tdmcp.perception.wrong_family",
        "message": f"unsupported capture mode {effective!r} for family {fam!r}",
        "path": getattr(node, "path", path),
        "mode": effective,
        "family": fam,
    }


def _maybe_downscale_top(td_mod, target, max_size: int):
    """Return ``(source_top, tmp_top|None)``; tmp must be destroyed by caller."""
    width = int(getattr(target, "width", 0) or 0)
    height = int(getattr(target, "height", 0) or 0)
    longest = max(width, height)
    if longest <= 0 or longest <= max_size:
        return target, None

    if width >= height:
        new_w = max_size
        new_h = max(1, round(height * max_size / width))
    else:
        new_h = max_size
        new_w = max(1, round(width * max_size / height))

    parent = target.parent()
    tmp_name = "__tdmcp_tmp_res__" + target.name
    existing = parent.op(tmp_name) if parent is not None else None
    if existing is not None:
        existing.destroy()
    tmp_top = parent.create(td_mod.resolutionTOP, tmp_name)
    tmp_top.inputConnectors[0].connect(target)
    tmp_top.par.outputresolution = "custom"
    tmp_top.par.resolutionw = new_w
    tmp_top.par.resolutionh = new_h
    return tmp_top, tmp_top


def _perception_frame_message(kind: str, mean_rgb: tuple[float, ...] | None) -> str:
    """Human message for black / uniform perception failures."""
    if mean_rgb is not None and len(mean_rgb) >= 3:
        rgb = f"{mean_rgb[0]:.2f},{mean_rgb[1]:.2f},{mean_rgb[2]:.2f}"
    elif mean_rgb is not None and mean_rgb:
        rgb = ",".join(f"{c:.2f}" for c in mean_rgb)
    else:
        rgb = None
    if kind == "black":
        base = "Captured TOP frame is black"
    else:
        base = "Captured TOP frame is a uniform solid color"
    if rgb is None:
        return f"{base} — perception fail"
    return f"{base} (mean rgb≈{rgb})"


def _classify_frame(
    target, data: bytes | None
) -> tuple[str | None, tuple[float, ...] | None]:
    """Classify a captured TOP as black / uniform / ok via `TOP.numpyArray`.

    Returns ``(kind, mean_rgb)`` where ``kind`` is ``"black"``, ``"uniform"``,
    or ``None`` (ok). ``mean_rgb`` is set when pixels were sampled.

    Black = mean RGB ≤ 1/255. Uniform = not black and per-channel spatial
    range (max−min) ≤ 2/255 (solid red counts; global max−min across channels
    would not). `saveByteArray` image size is not a reliable color signal —
    solid colors of *any* value compress similarly. Falls back to the old
    tiny-file heuristic as **black** only when `numpyArray` isn't available
    or produced no bytes — size alone cannot distinguish white from black.
    """
    if hasattr(target, "numpyArray"):
        try:
            arr = target.numpyArray(delayed=False)
            if arr is not None and arr.size:
                channels = min(3, arr.shape[-1]) if arr.ndim >= 3 else 1
                sample = arr[..., :channels] if arr.ndim >= 3 else arr
                mean_val = float(sample.mean())
                if sample.ndim >= 3:
                    ch_means = sample.mean(axis=(0, 1))
                    mean_rgb = tuple(float(x) for x in ch_means[:channels])
                    # Per-channel spatial range — solid red is uniform even though
                    # R≠G (global max−min across all samples would be ~1.0).
                    ch_max = sample.max(axis=(0, 1))
                    ch_min = sample.min(axis=(0, 1))
                    rng = max(
                        float(ch_max[i] - ch_min[i]) for i in range(channels)
                    )
                else:
                    mean_rgb = (mean_val,)
                    rng = float(sample.max() - sample.min())
                if mean_val <= _BLACK_MEAN_THRESHOLD:
                    return "black", mean_rgb
                if rng <= _UNIFORM_RANGE_THRESHOLD:
                    return "uniform", mean_rgb
                return None, mean_rgb
        except Exception:  # noqa: BLE001 — fall through to byte-size heuristic
            pass
    if not data or len(data) < 200:
        return "black", None
    return None, None


