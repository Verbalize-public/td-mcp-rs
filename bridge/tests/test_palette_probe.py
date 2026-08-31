"""Unit tests for the palette_probe seam (no live TD required)."""

from __future__ import annotations

import os
import sys
import unittest
from typing import Any

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402


class FakePar:
    def __init__(self, name: str, style: str = "Float", default: Any = 0.0) -> None:
        self.name = name
        self.label = name.title()
        self.style = style
        self.default = default
        self.menuNames: list[str] | None = None


class FakePage:
    def __init__(self, name: str, pars: list[FakePar]) -> None:
        self.name = name
        self.pars = pars


class FakeComp:
    """Minimal stand-in for a loaded palette COMP."""

    def __init__(
        self,
        *,
        name: str = "comp",
        op_type: str = "baseCOMP",
        children: list[Any] | None = None,
        pages: list[FakePage] | None = None,
        comment: str | None = None,
        extensions: list[Any] | None = None,
        errors: str = "",
        custom_pars: list[FakePar] | None = None,
    ) -> None:
        self.name = name
        self.opType = op_type
        self.family = "COMP"
        self.children = children or []
        self.customPages = pages or []
        self.customPars = custom_pars or []
        self.comment = comment
        self.extensions = extensions or []
        self.tags = []
        self.destroyed = False
        self._errors = errors

    def errors(self) -> str:
        return self._errors

    def destroy(self) -> None:
        self.destroyed = True


class FakeChild:
    def __init__(self, name: str, op_type: str, comment: str | None = None) -> None:
        self.name = name
        self.opType = op_type
        self.family = op_type[-3:].upper()
        self.comment = comment


class FakeExt:
    def Reset(self) -> None:  # noqa: N802 — TD extension methods are PascalCase
        pass

    def _private(self) -> None:
        pass


class FakeHelpDat:
    def __init__(self, text: str) -> None:
        self.name = "help"
        self.opType = "textDAT"
        self.family = "DAT"
        self.comment = None
        self.text = text


def wrapped_palette_tox(name: str = "particlesGpu") -> FakeComp:
    """The shape every stock palette `.tox` actually has.

    A bare wrapper with **no** custom parameters, holding an icon, a help DAT,
    and the real component as a COMP child named exactly like the wrapper.
    Verified live across 7 palette categories.
    """
    payload = particles_comp()
    payload.name = name
    return FakeComp(
        name=name,
        children=[
            FakeChild("icon", "nullTOP"),
            FakeHelpDat("particlesGpu\n\nGPU particle system."),
            payload,
        ],
    )


def particles_comp() -> FakeComp:
    return FakeComp(
        name="particlesGpu",
        op_type="containerCOMP",
        custom_pars=[FakePar("Birthrate"), FakePar("Reset", "Pulse")],
        children=[
            FakeChild("in1", "inTOP"),
            FakeChild("out1", "outTOP"),
            FakeChild("noise1", "noiseTOP", comment="base plate"),
            FakeChild("instances", "geometryCOMP"),
        ],
        pages=[
            FakePage(
                "Particles",
                [
                    FakePar("Birthrate", "Float", 1000.0),
                    FakePar("Reset", "Pulse", None),
                ],
            )
        ],
        comment="GPU particle system",
        extensions=[FakeExt()],
    )


class FakeProbeCtx(tdmcp_bridge.ProbeContext):
    def __init__(self, comps: dict[str, Any] | None = None) -> None:
        self.comps = comps or {}
        self.scratch_comp = FakeComp(op_type="baseCOMP")
        self.scratch_calls = 0
        self.loaded: list[Any] = []
        self.scratch_fails = False

    def scratch(self) -> Any:
        self.scratch_calls += 1
        if self.scratch_fails:
            return None
        return self.scratch_comp

    def load_tox(self, parent: Any, tox_path: str) -> Any | None:
        entry = self.comps.get(tox_path, "missing")
        if entry == "missing":
            return None
        if isinstance(entry, Exception):
            raise entry
        self.loaded.append(entry)
        return entry


class DigestTest(unittest.TestCase):
    def test_digest_captures_the_interface_not_the_internals(self) -> None:
        d = tdmcp_bridge.build_probe_digest(
            particles_comp(), "builtin:Tools/particlesGpu", "/p/particlesGpu.tox"
        )
        self.assertTrue(d["ok"])
        self.assertEqual(d["opType"], "containerCOMP")
        self.assertEqual(d["comment"], "GPU particle system")
        # Custom pars are the control API.
        self.assertEqual(d["customPars"][0]["page"], "Particles")
        names = [p["name"] for p in d["customPars"][0]["pars"]]
        self.assertEqual(names, ["Birthrate", "Reset"])
        self.assertEqual(d["customPars"][0]["pars"][1]["style"], "Pulse")
        # In/Out operators are the wiring boundary.
        self.assertEqual([p["name"] for p in d["inputs"]], ["in1"])
        self.assertEqual([p["name"] for p in d["outputs"]], ["out1"])
        # A COMP child is not a pin, even though "geometryCOMP" is neither.
        self.assertEqual(d["childCount"], 4)
        self.assertEqual(d["extensions"][0]["class"], "FakeExt")
        self.assertEqual(d["extensions"][0]["methods"], ["Reset"])

    def test_digest_survives_a_comp_that_answers_nothing(self) -> None:
        # Probing runs arbitrary components; a hostile one must not raise.
        class Hostile:
            @property
            def children(self) -> Any:
                raise RuntimeError("nope")

            @property
            def customPages(self) -> Any:
                raise RuntimeError("nope")

            @property
            def opType(self) -> Any:
                raise RuntimeError("nope")

        d = tdmcp_bridge.build_probe_digest(Hostile(), "user:X/y", "/p/y.tox")
        self.assertTrue(d["ok"])
        self.assertEqual(d["childCount"], 0)
        self.assertEqual(d["customPars"], [])

    def test_roster_is_capped_and_says_so(self) -> None:
        limit = tdmcp_bridge.PALETTE_CHILD_ROSTER_LIMIT
        comp = FakeComp(
            children=[FakeChild(f"n{i}", "noiseTOP") for i in range(limit + 5)]
        )
        d = tdmcp_bridge.build_probe_digest(comp, "user:X/y", "/p/y.tox")
        self.assertEqual(len(d["children"]), limit)
        self.assertEqual(d["childCount"], limit + 5)
        self.assertTrue(d["childrenTruncated"])


class RunProbeTest(unittest.TestCase):
    def targets(self, *paths: str) -> list[dict[str, Any]]:
        return [{"paletteId": f"builtin:T/{i}", "toxPath": p} for i, p in enumerate(paths)]

    def test_each_component_is_destroyed_and_so_is_the_scratch(self) -> None:
        a, b = particles_comp(), particles_comp()
        ctx = FakeProbeCtx({"/p/a.tox": a, "/p/b.tox": b})
        out = tdmcp_bridge.run_probe(ctx, self.targets("/p/a.tox", "/p/b.tox"), "detailed")
        self.assertTrue(out["ok"])
        self.assertEqual(len(out["results"]), 2)
        self.assertTrue(a.destroyed and b.destroyed)
        self.assertTrue(ctx.scratch_comp.destroyed, "scratch COMP must not survive")

    def test_a_failing_component_is_one_bad_row_not_a_failed_batch(self) -> None:
        good = particles_comp()
        ctx = FakeProbeCtx(
            {
                "/p/bad.tox": RuntimeError("socket timeout"),
                "/p/good.tox": good,
            }
        )
        out = tdmcp_bridge.run_probe(ctx, self.targets("/p/bad.tox", "/p/good.tox"), "summary")
        self.assertTrue(out["ok"], "one hostile component must not sink the batch")
        self.assertFalse(out["results"][0]["ok"])
        self.assertEqual(out["results"][0]["code"], "tdmcp.palette.probe_failed")
        self.assertIn("socket timeout", out["results"][0]["message"])
        self.assertTrue(out["results"][1]["ok"])
        self.assertTrue(good.destroyed)

    def test_the_scratch_is_destroyed_even_when_a_load_raises(self) -> None:
        ctx = FakeProbeCtx({"/p/bad.tox": RuntimeError("boom")})
        tdmcp_bridge.run_probe(ctx, self.targets("/p/bad.tox"), "summary")
        self.assertTrue(ctx.scratch_comp.destroyed)

    def test_a_load_that_produces_nothing_is_a_load_failure(self) -> None:
        ctx = FakeProbeCtx({})
        out = tdmcp_bridge.run_probe(ctx, self.targets("/p/gone.tox"), "summary")
        self.assertEqual(out["results"][0]["code"], "tdmcp.palette.load_failed")

    def test_summary_omits_internals_that_rarely_decide_a_pick(self) -> None:
        ctx = FakeProbeCtx({"/p/a.tox": particles_comp()})
        summary = tdmcp_bridge.run_probe(ctx, self.targets("/p/a.tox"), "summary")
        self.assertNotIn("children", summary["results"][0])
        self.assertNotIn("extensions", summary["results"][0])
        # The interface always survives — that is what the card is written from.
        self.assertIn("customPars", summary["results"][0])
        self.assertIn("inputs", summary["results"][0])
        self.assertEqual(summary["results"][0]["childCount"], 4)

        ctx = FakeProbeCtx({"/p/a.tox": particles_comp()})
        detailed = tdmcp_bridge.run_probe(ctx, self.targets("/p/a.tox"), "detailed")
        self.assertIn("children", detailed["results"][0])

    def test_batch_is_capped(self) -> None:
        limit = tdmcp_bridge.PALETTE_PROBE_BATCH_LIMIT
        comps = {f"/p/{i}.tox": particles_comp() for i in range(limit + 3)}
        ctx = FakeProbeCtx(comps)
        out = tdmcp_bridge.run_probe(ctx, self.targets(*comps), "summary")
        self.assertEqual(len(out["results"]), limit)

    def test_no_scratch_is_a_probe_failure_not_a_crash(self) -> None:
        ctx = FakeProbeCtx({})
        ctx.scratch_fails = True
        out = tdmcp_bridge.run_probe(ctx, self.targets("/p/a.tox"), "summary")
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.palette.probe_failed")


class HandlerTest(unittest.TestCase):
    def test_empty_targets_is_rejected(self) -> None:
        out = tdmcp_bridge.handle_palette_probe({"targets": []})
        self.assertFalse(out["ok"])
        self.assertEqual(out["code"], "tdmcp.palette.probe_failed")

    def test_palette_probe_is_a_registered_bridge_method(self) -> None:
        self.assertIn("palette_probe", tdmcp_bridge.HANDLERS)
        self.assertIn("palette_probe", tdmcp_bridge.BRIDGE_METHODS)


if __name__ == "__main__":
    unittest.main()


class WrapperUnwrapTest(unittest.TestCase):
    """Every stock palette `.tox` is a wrapper — verified live, 20/20 across
    Tools / Techniques / UI / Generators / ImageFilters / Mapping / POPs."""

    def test_payload_is_the_self_named_comp_child(self) -> None:
        wrapper = wrapped_palette_tox()
        payload = tdmcp_bridge.palette_payload(wrapper)
        self.assertIsNotNone(payload)
        self.assertEqual(payload.name, "particlesGpu")
        self.assertEqual(payload.opType, "containerCOMP")

    def test_a_users_own_tox_is_never_unwrapped(self) -> None:
        # Saved straight from a COMP: it has its own parameters and no
        # self-named child, so it is the component, not a wrapper.
        own = particles_comp()
        self.assertIsNone(tdmcp_bridge.palette_payload(own))

    def test_a_wrapper_lookalike_without_a_name_match_is_not_unwrapped(self) -> None:
        odd = FakeComp(
            name="mine",
            children=[FakeChild("icon", "nullTOP"), FakeChild("guts", "containerCOMP")],
        )
        self.assertIsNone(tdmcp_bridge.palette_payload(odd))

    def test_a_non_comp_name_match_is_not_a_payload(self) -> None:
        odd = FakeComp(name="thing", children=[FakeChild("thing", "textDAT")])
        self.assertIsNone(tdmcp_bridge.palette_payload(odd))

    def test_digest_describes_the_payload_not_the_wrapper(self) -> None:
        d = tdmcp_bridge.build_probe_digest(
            wrapped_palette_tox(), "builtin:Tools/particlesGpu", "/p/particlesGpu.tox"
        )
        self.assertTrue(d["wrapped"])
        # The wrapper has no parameters and no pins; the payload has both.
        self.assertEqual(d["opType"], "containerCOMP")
        self.assertEqual([p["name"] for p in d["customPars"][0]["pars"]],
                         ["Birthrate", "Reset"])
        self.assertEqual([p["name"] for p in d["inputs"]], ["in1"])
        # The wrapper's help DAT is the component's own documentation.
        self.assertIn("GPU particle system", d["help"])

    def test_digest_of_an_unwrapped_component_reports_no_wrapper(self) -> None:
        d = tdmcp_bridge.build_probe_digest(
            particles_comp(), "user:Mine/thing", "/p/thing.tox"
        )
        self.assertNotIn("wrapped", d)
        self.assertNotIn("help", d)

    def test_empty_extension_slots_are_not_reported_as_a_class(self) -> None:
        # TD lists an un-extended COMP's slot as None; "NoneType" is noise.
        comp = particles_comp()
        comp.extensions = [None, None]
        d = tdmcp_bridge.build_probe_digest(comp, "user:X/y", "/p/y.tox")
        self.assertNotIn("extensions", d)


class ThumbnailProbeCtx(FakeProbeCtx):
    """Probe context that answers the thumbnail seam with a canned shot."""

    def __init__(self, comps: dict[str, Any] | None = None, shot: Any = None) -> None:
        super().__init__(comps)
        self.shot = shot
        self.thumbnail_targets: list[Any] = []

    def thumbnail(self, loaded: Any, comp: Any) -> Any:
        self.thumbnail_targets.append((loaded, comp))
        if isinstance(self.shot, Exception):
            raise self.shot
        return self.shot


class ThumbnailTest(unittest.TestCase):
    def test_wrapper_icon_is_found_and_absent_when_unwrapped(self) -> None:
        self.assertEqual(
            tdmcp_bridge.wrapper_icon(wrapped_palette_tox()).opType, "nullTOP"
        )
        # A component saved straight from a COMP ships no icon child.
        self.assertIsNone(tdmcp_bridge.wrapper_icon(particles_comp()))

    def test_thumbnail_rides_the_digest_when_asked(self) -> None:
        ctx = ThumbnailProbeCtx(
            {"/p/w.tox": wrapped_palette_tox()},
            shot={"ok": True, "imageBase64": "QUJD", "mimeType": "image/png"},
        )
        out = tdmcp_bridge.run_probe(
            ctx,
            [{"paletteId": "builtin:Tools/w", "toxPath": "/p/w.tox"}],
            "summary",
            thumbnails=True,
        )
        row = out["results"][0]
        self.assertTrue(row["ok"])
        self.assertEqual(row["thumbnailBase64"], "QUJD")
        self.assertEqual(row["thumbnailMime"], "image/png")
        self.assertNotIn("thumbnailNote", row)

    def test_the_payload_is_what_gets_rendered_not_the_wrapper(self) -> None:
        # Same unwrap law as the digest: the wrapper is an icon in a box.
        ctx = ThumbnailProbeCtx(
            {"/p/w.tox": wrapped_palette_tox()},
            shot={"imageBase64": "QUJD"},
        )
        tdmcp_bridge.run_probe(
            ctx,
            [{"paletteId": "builtin:Tools/w", "toxPath": "/p/w.tox"}],
            "summary",
            thumbnails=True,
        )
        loaded, comp = ctx.thumbnail_targets[0]
        self.assertEqual(loaded.opType, "baseCOMP")       # the wrapper
        self.assertEqual(comp.opType, "containerCOMP")    # the real component

    def test_a_frame_that_drew_nothing_is_reported_not_stored(self) -> None:
        # An all-black tile in the UI reads as a bug; the placeholder does not.
        for code in ("tdmcp.perception.black_frame", "tdmcp.perception.uniform_frame"):
            with self.subTest(code=code):
                ctx = ThumbnailProbeCtx(
                    {"/p/w.tox": wrapped_palette_tox()},
                    shot={"ok": False, "code": code, "imageBase64": "QUJD"},
                )
                out = tdmcp_bridge.run_probe(
                    ctx,
                    [{"paletteId": "builtin:Tools/w", "toxPath": "/p/w.tox"}],
                    "summary",
                    thumbnails=True,
                )
                row = out["results"][0]
                self.assertTrue(row["ok"])
                self.assertEqual(row["thumbnailNote"], code)
                self.assertNotIn("thumbnailBase64", row)

    def test_a_thumbnail_failure_never_downgrades_the_row(self) -> None:
        for shot in (RuntimeError("gpu is on fire"), None, {}, {"imageBase64": ""}):
            with self.subTest(shot=shot):
                ctx = ThumbnailProbeCtx(
                    {"/p/w.tox": wrapped_palette_tox()}, shot=shot
                )
                out = tdmcp_bridge.run_probe(
                    ctx,
                    [{"paletteId": "builtin:Tools/w", "toxPath": "/p/w.tox"}],
                    "summary",
                    thumbnails=True,
                )
                row = out["results"][0]
                self.assertTrue(row["ok"])
                self.assertEqual(row["opType"], "containerCOMP")
                self.assertNotIn("thumbnailBase64", row)

    def test_thumbnails_are_off_by_default(self) -> None:
        ctx = ThumbnailProbeCtx(
            {"/p/w.tox": wrapped_palette_tox()}, shot={"imageBase64": "QUJD"}
        )
        out = tdmcp_bridge.run_probe(
            ctx, [{"paletteId": "builtin:Tools/w", "toxPath": "/p/w.tox"}], "summary"
        )
        self.assertNotIn("thumbnailBase64", out["results"][0])
        self.assertEqual(ctx.thumbnail_targets, [])
