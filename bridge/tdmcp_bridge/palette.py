"""``palette_probe`` — load Palette components, digest their interface, destroy.

The daemon picks the batch and resolves every ``.tox`` path; this handler only
answers "what does this component look like from the outside". Load → digest →
destroy happens in one main-thread task so an abandoned call can never leave a
half-loaded component in the user's project.

The digest is **evidence**, not a card: the agent turns it into a summary and an
OpSketch via ``palette_index action=describe``.
"""

from __future__ import annotations

import time
from typing import Any

from .constants import (
    COMMENT_MAX_CHARS,
    PALETTE_CHILD_ROSTER_LIMIT,
    PALETTE_HELP_MAX_CHARS,
    PALETTE_PAR_PAGE_LIMIT,
    PALETTE_PARS_PER_PAGE_LIMIT,
    PALETTE_PROBE_BATCH_LIMIT,
    PALETTE_SCRATCH_NAME,
    PALETTE_THUMB_MAX_SIZE,
)

# Child opType prefixes that form a COMP's wiring boundary.
_IN_PREFIX = "in"
_OUT_PREFIX = "out"

# Decoration children of a palette wrapper (not the component itself).
_WRAPPER_DECOR = ("icon", "help")

# Capture verdicts that mean "nothing was drawn". A thumbnail is a picture or
# it is nothing; an all-black tile only looks like a bug.
_EMPTY_FRAME_CODES = (
    "tdmcp.perception.black_frame",
    "tdmcp.perception.uniform_frame",
)


def palette_payload(loaded: Any) -> Any | None:
    """The real component inside a Palette wrapper, or ``None`` if not wrapped.

    Every stock palette ``.tox`` is a wrapper: a bare ``baseCOMP`` holding an
    ``icon``, an optional ``help`` DAT, and the actual component as a COMP child
    named exactly like the wrapper. Verified across Tools / Techniques / UI /
    Generators / ImageFilters / Mapping / POPs — the wrapper carries **zero**
    custom parameters while the payload carries 3–65 of them, so digesting or
    placing the wrapper would hand back a component with no API and an icon
    stapled to it.

    Detection is deliberately narrow so a user's own ``.tox`` — saved straight
    from a COMP, with its own parameters and no self-named child — is never
    unwrapped by mistake.
    """
    name = _attr(loaded, "name")
    if not name:
        return None
    try:
        if list(_attr(loaded, "customPars", None) or []):
            return None  # Has its own API: it is the component, not a wrapper.
    except Exception:  # noqa: BLE001
        return None
    for child in _children(loaded):
        if _attr(child, "name") != name:
            continue
        if _text(_attr(child, "family"), 8) != "COMP":
            continue
        return child
    return None


def wrapper_help(loaded: Any) -> str | None:
    """Text of a palette wrapper's ``help`` DAT — the component's own docs."""
    for child in _children(loaded):
        if _attr(child, "name") == "help":
            return _text(_attr(child, "text"), PALETTE_HELP_MAX_CHARS)
    return None


def wrapper_icon(loaded: Any) -> Any | None:
    """A palette wrapper's ``icon`` child — Derivative's own artwork.

    This is the tile TouchDesigner itself shows in its palette browser, already
    composed and already the right shape for a thumbnail. Preferring it over
    rendering the component means the common case never cooks a stranger's
    graph just to draw a picture.
    """
    for child in _children(loaded):
        if _attr(child, "name") == "icon":
            return child
    return None


def _children(node: Any) -> list[Any]:
    """Direct children as a list; never raises."""
    try:
        return list(_attr(node, "children", None) or [])
    except Exception:  # noqa: BLE001
        return []


def _attr(node: Any, name: str, default: Any = None) -> Any:
    """``getattr`` that survives a raising property.

    A probe loads arbitrary third-party components; any of their members may be
    a property that throws. Nothing here is a claim worth failing a batch over.
    """
    try:
        return getattr(node, name, default)
    except Exception:  # noqa: BLE001
        return default


def _text(value: Any, limit: int = COMMENT_MAX_CHARS) -> str | None:
    """Trimmed string, capped; ``None`` when empty or unreadable."""
    try:
        if value is None:
            return None
        s = str(value).strip()
    except Exception:  # noqa: BLE001
        return None
    if not s:
        return None
    return s[:limit] if len(s) > limit else s


def _par_entry(par: Any) -> dict[str, Any]:
    """One custom parameter: how to drive it, not its current value."""
    entry: dict[str, Any] = {
        "name": _text(_attr(par, "name", None), 64),
        "label": _text(_attr(par, "label", None), 128),
        "style": _text(_attr(par, "style", None), 32),
    }
    default = _attr(par, "default")
    if not isinstance(default, (str, int, float, bool, type(None))):
        default = _text(default, 128)
    entry["default"] = default
    menu_names = _attr(par, "menuNames", None)
    if menu_names:
        try:
            entry["menuNames"] = [str(m) for m in list(menu_names)[:32]]
        except Exception:  # noqa: BLE001
            pass
    return entry


def _custom_pars(comp: Any) -> list[dict[str, Any]]:
    """Custom parameters grouped by page — the component's control API."""
    pages: list[dict[str, Any]] = []
    try:
        raw_pages = list(_attr(comp, "customPages", None) or [])
    except Exception:  # noqa: BLE001
        raw_pages = []
    for page in raw_pages[:PALETTE_PAR_PAGE_LIMIT]:
        try:
            pars = list(_attr(page, "pars", None) or [])
        except Exception:  # noqa: BLE001
            pars = []
        pages.append({
            "page": _text(_attr(page, "name", None), 64),
            "pars": [_par_entry(p) for p in pars[:PALETTE_PARS_PER_PAGE_LIMIT]],
        })
    return pages


def _pins(comp: Any) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """In/Out operators — the wiring boundary a caller connects to."""
    inputs: list[dict[str, Any]] = []
    outputs: list[dict[str, Any]] = []
    for child in _children(comp):
        op_type = _text(_attr(child, "opType", None), 64) or ""
        row = {
            "name": _text(_attr(child, "name", None), 64),
            "opType": op_type,
            "family": _text(_attr(child, "family", None), 8),
        }
        # inTOP / outCHOP / … — the family suffix is what makes them pins.
        if op_type.startswith(_IN_PREFIX) and op_type[len(_IN_PREFIX):].isupper():
            inputs.append(row)
        elif op_type.startswith(_OUT_PREFIX) and op_type[len(_OUT_PREFIX):].isupper():
            outputs.append(row)
    return inputs, outputs


def _roster(comp: Any) -> tuple[list[dict[str, Any]], int, bool]:
    """Depth-1 child roster, capped. Returns ``(rows, total, truncated)``."""
    children = _children(comp)
    total = len(children)
    rows = []
    for child in children[:PALETTE_CHILD_ROSTER_LIMIT]:
        row: dict[str, Any] = {
            "name": _text(_attr(child, "name", None), 64),
            "opType": _text(_attr(child, "opType", None), 64),
        }
        comment = _text(_attr(child, "comment", None), 160)
        if comment:
            row["comment"] = comment
        rows.append(row)
    return rows, total, total > PALETTE_CHILD_ROSTER_LIMIT


def _extensions(comp: Any) -> list[dict[str, Any]]:
    """Promoted Python extensions: class name + public methods."""
    out: list[dict[str, Any]] = []
    try:
        exts = list(_attr(comp, "extensions", None) or [])
    except Exception:  # noqa: BLE001
        return out
    for ext in exts[:8]:
        if ext is None:
            continue  # TD lists an empty extension slot as None — not a class.
        try:
            cls = type(ext).__name__
            members = [
                m
                for m in dir(ext)
                if not m.startswith("_") and callable(_attr(ext, m))
            ]
        except Exception:  # noqa: BLE001
            continue
        out.append({"class": cls, "methods": members[:64]})
    return out


def build_probe_digest(
    loaded: Any,
    palette_id: str,
    tox_path: str,
    *,
    unwrap: bool = True,
) -> dict[str, Any]:
    """Shape one loaded component into evidence. Pure — no ``td`` import.

    ``loaded`` is whatever ``loadTox`` returned. When that is a palette wrapper
    the digest describes the **payload** — the wrapper has no parameters and no
    pins, so digesting it would describe an icon.
    """
    comp = loaded
    payload = palette_payload(loaded) if unwrap else None
    help_text = None
    if payload is not None:
        help_text = wrapper_help(loaded)
        comp = payload
    inputs, outputs = _pins(comp)
    children, child_count, truncated = _roster(comp)
    digest: dict[str, Any] = {
        "ok": True,
        "paletteId": palette_id,
        "toxPath": tox_path,
        "opType": _text(_attr(comp, "opType", None), 64),
        "family": _text(_attr(comp, "family", None), 8),
        "customPars": _custom_pars(comp),
        "inputs": inputs,
        "outputs": outputs,
        "children": children,
        "childCount": child_count,
    }
    if truncated:
        digest["childrenTruncated"] = True
    if payload is not None:
        digest["wrapped"] = True
    if help_text:
        digest["help"] = help_text
    comment = _text(_attr(comp, "comment", None))
    if comment:
        digest["comment"] = comment
    try:
        tags = list(_attr(comp, "tags", None) or [])
        if tags:
            digest["tags"] = [str(t) for t in tags[:32]]
    except Exception:  # noqa: BLE001
        pass
    extensions = _extensions(comp)
    if extensions:
        digest["extensions"] = extensions
    errors = _text(_call_or_none(_attr(comp, "errors", None)))
    if errors:
        digest["errors"] = errors
    return digest


def _call_or_none(fn: Any) -> Any:
    """Call a zero-arg TD accessor; ``None`` on absence or failure."""
    if not callable(fn):
        return None
    try:
        return fn()
    except Exception:  # noqa: BLE001
        return None


class ProbeContext:
    """Load/destroy hooks for :func:`run_probe` — no ``td`` import in the seam."""

    def scratch(self) -> Any:
        """A hidden COMP to load components under. Created on demand."""
        raise NotImplementedError

    def load_tox(self, parent: Any, tox_path: str) -> Any | None:
        """Load a ``.tox`` under ``parent``; returns the loaded COMP."""
        raise NotImplementedError

    def destroy(self, node: Any) -> None:
        """Best-effort destroy; never raises."""
        try:
            fn = _attr(node, "destroy", None)
            if callable(fn):
                fn()
        except Exception:  # noqa: BLE001
            pass

    def thumbnail(self, loaded: Any, comp: Any) -> dict[str, Any] | None:
        """PNG preview of one loaded component, or ``None`` when unavailable.

        ``loaded`` is what ``loadTox`` produced (the wrapper, for stock
        components); ``comp`` is the unwrapped payload. Implementations return
        the capture-shaped dict (``imageBase64`` / ``mimeType`` / ``code``).
        """
        return None


def run_probe(
    ctx: ProbeContext,
    targets: list[dict[str, Any]],
    detail_level: str,
    *,
    thumbnails: bool = False,
) -> dict[str, Any]:
    """Load, digest, and destroy each target. Partial success per component.

    A component that fails to load becomes an error row — never a failed batch,
    so one hostile component costs the caller one row, not the whole run. The
    scratch COMP is destroyed even when a load raises.

    With ``thumbnails`` the digest also carries a small PNG. It is rendered in
    the same load→destroy window (the component only exists there) and is
    strictly best-effort: a missing picture never downgrades a row.
    """
    results: list[dict[str, Any]] = []
    scratch = None
    try:
        scratch = ctx.scratch()
        if scratch is None:
            return {
                "ok": False,
                "code": "tdmcp.palette.probe_failed",
                "message": "could not create the probe scratch COMP",
            }
        for target in targets[:PALETTE_PROBE_BATCH_LIMIT]:
            palette_id = str(target.get("paletteId") or "")
            tox_path = str(target.get("toxPath") or "")
            started = time.monotonic()
            loaded = None
            try:
                loaded = ctx.load_tox(scratch, tox_path)
                if loaded is None:
                    results.append({
                        "ok": False,
                        "paletteId": palette_id,
                        "code": "tdmcp.palette.load_failed",
                        "message": f"loadTox produced no component from {tox_path}",
                    })
                    continue
                # Digest unwraps; destroy still targets what loadTox created,
                # since the wrapper owns the payload.
                digest = build_probe_digest(loaded, palette_id, tox_path)
                if thumbnails:
                    _attach_thumbnail(ctx, digest, loaded)
                digest["probeMs"] = int((time.monotonic() - started) * 1000)
                if detail_level != "detailed":
                    # Internals are the expensive half and rarely decide a pick.
                    digest.pop("children", None)
                    digest.pop("extensions", None)
                results.append(digest)
            except Exception as exc:  # noqa: BLE001 — one bad component, one bad row
                results.append({
                    "ok": False,
                    "paletteId": palette_id,
                    "code": "tdmcp.palette.probe_failed",
                    "message": f"{type(exc).__name__}: {exc}",
                })
            finally:
                if loaded is not None:
                    ctx.destroy(loaded)
    finally:
        if scratch is not None:
            ctx.destroy(scratch)
    return {"ok": True, "results": results, "scratchName": PALETTE_SCRATCH_NAME}


def _attach_thumbnail(ctx: ProbeContext, digest: dict[str, Any], loaded: Any) -> None:
    """Render a thumbnail into ``digest``; swallow every failure.

    A probe runs arbitrary third-party code and the thumbnail is the least
    important thing it produces — anything that raises here leaves the digest
    exactly as it was, with no key added and no row downgraded.
    """
    try:
        payload = palette_payload(loaded)
        shot = ctx.thumbnail(loaded, payload if payload is not None else loaded)
    except Exception:  # noqa: BLE001
        return
    if not isinstance(shot, dict):
        return
    b64 = shot.get("imageBase64")
    if not b64:
        return
    note = shot.get("code")
    if note in _EMPTY_FRAME_CODES:
        # A component that did not draw is the common case for an unwrapped
        # `.tox`: the OP Viewer has not rasterized by the time this task saves
        # it. Storing the black rectangle would put a broken-looking tile in
        # front of the user; reporting the code instead lets the caller fall
        # back to its own placeholder, which is an honest "no preview yet".
        digest["thumbnailNote"] = note
        return
    digest["thumbnailBase64"] = b64
    digest["thumbnailMime"] = shot.get("mimeType") or "image/png"
    if note:
        digest["thumbnailNote"] = note


class _TdProbeContext(ProbeContext):
    """Live TD: a hidden scratch COMP under ``/``."""

    def scratch(self) -> Any:
        import td  # type: ignore

        root = td.op("/")
        if root is None:
            return None
        existing = root.op(PALETTE_SCRATCH_NAME)
        if existing is not None:
            # A leftover from an aborted run — start clean rather than digest
            # whatever it still holds.
            try:
                existing.destroy()
            except Exception:  # noqa: BLE001
                return existing
        comp = root.create(td.baseCOMP, PALETTE_SCRATCH_NAME)
        for flag in ("display", "viewer"):
            try:
                setattr(comp.par, flag, False)
            except Exception:  # noqa: BLE001 — cosmetic only
                pass
        return comp

    def load_tox(self, parent: Any, tox_path: str) -> Any | None:
        load_fn = _attr(parent, "loadTox", None)
        if not callable(load_fn):
            raise AttributeError("scratch COMP has no loadTox")
        before = {id(c) for c in list(_attr(parent, "children", []) or [])}
        loaded = load_fn(tox_path)
        if loaded is not None:
            return loaded
        for child in list(_attr(parent, "children", []) or []):
            if id(child) not in before:
                return child
        return None

    def thumbnail(self, loaded: Any, comp: Any) -> dict[str, Any] | None:
        """Wrapper ``icon`` first, the component's own viewer as a fallback.

        The icon is a plain TOP, so it saves without cooking the component at
        all — that is the whole reason it is tried first. Only an unwrapped
        ``.tox`` (someone's own component, which ships no icon) pays for a
        render through the shared OP Viewer.
        """
        import td  # type: ignore

        from . import capture as _capture

        path = _text(_attr(comp, "path", None), 256) or ""

        icon = wrapper_icon(loaded)
        if icon is not None and hasattr(icon, "saveByteArray"):
            shot = _capture._capture_top_image(
                td, icon, path, PALETTE_THUMB_MAX_SIZE
            )
            if shot.get("imageBase64"):
                return shot

        # No icon (or it produced nothing): rasterize the component itself.
        # Safe under the per-pid FIFO — see the note on the shared viewer.
        return _capture._capture_via_shared_viewer(
            td, comp, path, PALETTE_THUMB_MAX_SIZE, mode="preview"
        )


def handle_palette_probe(params: dict[str, Any]) -> dict[str, Any]:
    """Bridge entrypoint for ``palette_probe``."""
    targets = params.get("targets") or []
    if not isinstance(targets, list) or not targets:
        return {
            "ok": False,
            "code": "tdmcp.palette.probe_failed",
            "message": "palette_probe requires a non-empty targets array",
        }
    detail_level = str(params.get("detailLevel") or "summary")
    thumbnails = bool(params.get("thumbnails"))
    return run_probe(
        _TdProbeContext(), targets, detail_level, thumbnails=thumbnails
    )
