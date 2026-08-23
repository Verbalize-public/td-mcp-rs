"""Unit tests for the shared shader-compile lint seam (no live TD required).

Classifier matrix uses the live-verified compileResult strings recorded in
docs/SHADER_LINT.md §2 (V2/V4/V5) verbatim.
"""

from __future__ import annotations

import os
import sys
import unittest
from types import SimpleNamespace
from typing import Any

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from tdmcp_bridge.shader_lint import (  # noqa: E402
    classify_compile_result,
    discover_consumers,
    lint_dat_consumers,
)

# docs/SHADER_LINT.md V4 — glslTOP success string.
SUCCESS_TOP = (
    "Vertex Shader Compile Results:\n\nCompiled Successfully\n\n"
    "=============\nPixel Shader Compile Results:\n\nCompiled Successfully\n"
)
# V4 — glslMAT appends the link section.
SUCCESS_MAT = SUCCESS_TOP + "\n=============\n\nLinked Successfully\n"
# V5 — failure lines carry full DAT path + line, last line is the summary.
FAILURE_TOP = (
    "Vertex Shader Compile Results:\n\n"
    "ERROR: /project1/probe/frag:5: '' : syntax error, unexpected RIGHT_BRACE\n"
    "ERROR: 1 compilation errors.  No code generated.\n"
)
FAILURE_LINES = [
    "ERROR: /project1/probe/frag:5: '' : syntax error, unexpected RIGHT_BRACE",
    "ERROR: 1 compilation errors.  No code generated.",
]


def _fake_dat(path: str, text: str = "") -> SimpleNamespace:
    return SimpleNamespace(path=path, opType="textDAT", text=text)


class _FakePar:
    def __init__(self, target: Any) -> None:
        self._target = target

    def eval(self) -> Any:
        if isinstance(self._target, Exception):
            raise self._target
        return self._target


class _FakeGlslOp:
    def __init__(
        self,
        path: str,
        op_type: str,
        *,
        stage_par: str = "pixeldat",
        dat: Any = None,
        compile_result: Any = None,
        has_result: bool = True,
    ) -> None:
        self.path = path
        self.opType = op_type
        self.par = SimpleNamespace(**{stage_par: _FakePar(dat)})
        if has_result:
            self.compileResult = compile_result


class FakeLintCtx:
    """Duck-typed MutateContext for discovery: resolve + find_children."""

    def __init__(self, root: Any | None = None) -> None:
        self.root = root if root is not None else SimpleNamespace(path="/project1")
        self.by_type: dict[str, list[Any]] = {}

    def register(self, type_name: str, *ops: Any) -> None:
        self.by_type.setdefault(type_name, []).extend(ops)

    def resolve(self, path: str) -> Any | None:
        return self.root if path == getattr(self.root, "path", path) else None

    def find_children(self, root: Any, type_name: str) -> list[Any]:
        return list(self.by_type.get(type_name, []))


class TestClassifyCompileResult(unittest.TestCase):
    def test_success_top_is_note_compiled(self) -> None:
        item = classify_compile_result("glslTOP", SUCCESS_TOP)
        self.assertEqual(item["severity"], "note")
        self.assertEqual(item["code"], "tdmcp.shader.compiled")
        self.assertIn("Compiled Successfully", item["message"])
        self.assertNotIn("lines", item)

    def test_success_mat_message_includes_link(self) -> None:
        item = classify_compile_result("glslMAT", SUCCESS_MAT)
        self.assertEqual(item["code"], "tdmcp.shader.compiled")
        self.assertIn("Linked Successfully", item["message"])

    def test_failure_lines_verbatim(self) -> None:
        item = classify_compile_result("glslTOP", FAILURE_TOP)
        self.assertEqual(item["severity"], "error")
        self.assertEqual(item["code"], "tdmcp.shader.compile_failed")
        self.assertEqual(item["lines"], FAILURE_LINES)

    def test_empty_string_counts_as_compiled(self) -> None:
        item = classify_compile_result("glslTOP", "")
        self.assertEqual(item["severity"], "note")
        self.assertEqual(item["code"], "tdmcp.shader.compiled")

    def test_missing_result_is_unsupported(self) -> None:
        item = classify_compile_result("glslTOP", None)
        self.assertEqual(item["severity"], "note")
        self.assertEqual(item["code"], "tdmcp.shader.unsupported_consumer")

    def test_glslpop_excluded_by_optype(self) -> None:
        item = classify_compile_result("glslPOP", "anything")
        self.assertEqual(item["severity"], "note")
        self.assertEqual(item["code"], "tdmcp.shader.unsupported_consumer")


class TestDiscoverConsumers(unittest.TestCase):
    def _dat(self) -> SimpleNamespace:
        return _fake_dat("/project1/probe/shader")

    def test_match_via_pixeldat_reports_role_and_status(self) -> None:
        dat = self._dat()
        glsl = _FakeGlslOp(
            "/project1/fx/glsl1",
            "glslTOP",
            dat=dat,
            compile_result=SUCCESS_TOP,
        )
        ctx = FakeLintCtx()
        ctx.register("glslTOP", glsl)
        out = discover_consumers(ctx, dat.path)
        self.assertEqual(len(out["consumers"]), 1)
        item = out["consumers"][0]
        self.assertEqual(item["consumer"], glsl.path)
        self.assertEqual(item["consumerOpType"], "glslTOP")
        self.assertEqual(item["role"], "pixel")
        self.assertEqual(item["code"], "tdmcp.shader.compiled")
        self.assertNotIn("consumersTruncated", out)

    def test_mat_stage_par_maps_to_pixel_role(self) -> None:
        dat = self._dat()
        mat = _FakeGlslOp(
            "/project1/m", "glslMAT", stage_par="pdat", dat=dat,
            compile_result=FAILURE_TOP,
        )
        ctx = FakeLintCtx()
        ctx.register("glslMAT", mat)
        out = discover_consumers(ctx, dat.path)
        item = out["consumers"][0]
        self.assertEqual(item["role"], "pixel")
        self.assertEqual(item["severity"], "error")
        self.assertEqual(item["lines"], FAILURE_LINES)

    def test_no_match_returns_empty(self) -> None:
        ctx = FakeLintCtx()
        out = discover_consumers(ctx, "/project1/nobody")
        self.assertEqual(out["consumers"], [])

    def test_multiple_consumers_all_reported(self) -> None:
        dat = self._dat()
        top = _FakeGlslOp("/project1/t", "glslTOP", dat=dat, compile_result=SUCCESS_TOP)
        mat = _FakeGlslOp(
            "/project1/m", "glslMAT", stage_par="pdat", dat=dat,
            compile_result=SUCCESS_MAT,
        )
        ctx = FakeLintCtx()
        ctx.register("glslTOP", top)
        ctx.register("glslMAT", mat)
        out = discover_consumers(ctx, dat.path)
        self.assertEqual(
            sorted(i["consumer"] for i in out["consumers"]),
            sorted([top.path, mat.path]),
        )

    def test_glslpop_consumer_is_unsupported_note(self) -> None:
        dat = self._dat()
        pop = _FakeGlslOp(
            "/project1/p", "glslPOP", stage_par="computedat", dat=dat,
            compile_result=None,
        )
        ctx = FakeLintCtx()
        ctx.register("glslPOP", pop)
        out = discover_consumers(ctx, dat.path)
        item = out["consumers"][0]
        self.assertEqual(item["code"], "tdmcp.shader.unsupported_consumer")
        self.assertEqual(item["role"], "compute")

    def test_consumer_cap_truncates_with_object(self) -> None:
        dat = self._dat()
        ops = [
            _FakeGlslOp(f"/project1/g{i}", "glslTOP", dat=dat, compile_result="")
            for i in range(4)
        ]
        ctx = FakeLintCtx()
        ctx.register("glslTOP", *ops)
        out = discover_consumers(ctx, dat.path, consumer_limit=2)
        self.assertEqual(len(out["consumers"]), 2)
        self.assertTrue(out["consumersTruncated"])
        self.assertEqual(out["truncation"]["code"], "tdmcp.shader.consumers_truncated")

    def test_scan_cap_truncates(self) -> None:
        dat = self._dat()
        noise = [SimpleNamespace(path=f"/project1/n{i}", opType="glslTOP") for i in range(6)]
        ctx = FakeLintCtx()
        ctx.register("glslTOP", *noise)
        out = discover_consumers(ctx, dat.path, scan_limit=3)
        self.assertTrue(out["consumersTruncated"])

    def test_both_caps_fired_limit_prefers_scan_branch(self) -> None:
        dat = self._dat()
        consumer_a = _FakeGlslOp("/project1/a", "glslTOP", dat=dat, compile_result="")
        filler = [SimpleNamespace(path=f"/project1/f{i}", opType="glslTOP") for i in range(3)]
        consumer_b = _FakeGlslOp("/project1/b", "glslTOP", dat=dat, compile_result="")
        tail = SimpleNamespace(path="/project1/tail", opType="glslTOP")
        ctx = FakeLintCtx()
        # a (kept) → f0 → b (overflow) → f1 → f2 (scanned=5 > scan_limit=4).
        ctx.register("glslTOP", consumer_a, filler[0], consumer_b, filler[1], filler[2], tail)
        out = discover_consumers(ctx, dat.path, scan_limit=4, consumer_limit=1)
        self.assertEqual(len(out["consumers"]), 1)
        self.assertTrue(out["consumersTruncated"])
        trunc = out["truncation"]
        self.assertEqual(trunc["limit"], 4, "limit must follow the scan branch when both caps fire")
        self.assertIn("scan capped at 4", trunc["message"])

    def test_one_family_raising_does_not_block_others(self) -> None:
        dat = self._dat()
        top = _FakeGlslOp("/project1/t", "glslTOP", dat=dat, compile_result="")

        class HalfBrokenCtx(FakeLintCtx):
            def find_children(self, root: Any, type_name: str) -> list[Any]:
                if type_name == "glslMAT":
                    raise RuntimeError("boom")
                return super().find_children(root, type_name)

        ctx = HalfBrokenCtx()
        ctx.register("glslTOP", top)
        out = discover_consumers(ctx, dat.path)
        self.assertEqual([i["consumer"] for i in out["consumers"]], [top.path])

    def test_unresolvable_scope_root_degrades_to_empty(self) -> None:
        ctx = FakeLintCtx()
        out = discover_consumers(ctx, "/project1/x", scope_root="/nope")
        self.assertEqual(out, {})


class TestLintDatConsumersNeverRaises(unittest.TestCase):
    def test_raising_ctx_returns_empty_dict(self) -> None:
        class ExplodingCtx:
            def resolve(self, path: str) -> Any:
                raise RuntimeError("no td")

        self.assertEqual(lint_dat_consumers(ExplodingCtx(), "/p"), {})

    def test_scope_root_defaults_to_project1(self) -> None:
        seen: dict[str, str] = {}

        class SpyCtx(FakeLintCtx):
            def resolve(self, path: str) -> Any | None:
                seen["scope"] = path
                return super().resolve(path)

        lint_dat_consumers(SpyCtx(), "/project1/d", None)
        self.assertEqual(seen["scope"], "/project1")


if __name__ == "__main__":
    unittest.main()
