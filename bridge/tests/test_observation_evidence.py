"""Unknown observations must never masquerade as clean/compiled results."""
import sys
from pathlib import Path
from types import SimpleNamespace as NS

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tdmcp_bridge.inspect import build_inspect_node


def node(**overrides):
    return NS(path="/project1/test", opType="nullTOP", family="TOP", children=[],
              **overrides)


def broken():
    raise RuntimeError("unreadable")


def test_empty_messages_are_distinct_from_unavailable():
    out = build_inspect_node(node(errors=broken, warnings=lambda: ""),
                             want_errors=True, want_warnings=True)
    assert out["errors"] == out["warnings"] == []
    assert out["observations"]["errors"]["available"] is False
    assert out["observations"]["warnings"] == {"available": True}
    assert out["observations"]["errors"]["code"] == "tdmcp.op.observation_unavailable"


def test_wire_iteration_failure_is_not_empty_graph():
    class BrokenEdges:
        def __iter__(self):
            raise RuntimeError("edges not readable")
    out = build_inspect_node(node(inputs=BrokenEdges(), outputs=[]))
    assert out["inputs"] == out["outputs"] == []
    assert not out["observations"]["inputs"]["available"]
    assert out["observations"]["outputs"]["available"]


def test_parameter_null_is_distinct_from_failed_evaluation():
    pars = [NS(name="empty", mode="CONSTANT", eval=lambda: None),
            NS(name="broken", mode="EXPRESSION", expr="1/0", eval=broken)]
    out = build_inspect_node(node(pars=lambda: pars), want_nodes=False, want_params=True)
    empty, failed = out["params"]
    assert empty["val"] is failed["val"] is None
    assert empty["evaluation"] == {"available": True}
    assert failed["evaluation"]["available"] is False
    assert failed["evaluation"]["errorType"] == "RuntimeError"
    assert failed["expr"] == "1/0"


def test_compile_property_failure_keeps_shader_and_other_evidence():
    class Shader:
        path = "/project1/shader"
        opType = "glslTOP"
        family = "TOP"
        par = NS()
        @property
        def compileResult(self):
            raise RuntimeError("compiler unavailable")
    out = build_inspect_node(Shader(), want_nodes=False, want_content=True)
    assert out["content"]["compileState"] == "unknown"
    assert out["content"]["compileDiagnostic"]["readError"]["type"] == "RuntimeError"


def test_messages_follow_compile_observation_in_the_same_reply():
    class Shader:
        path = "/project1/shader"
        opType = "glslTOP"
        family = "TOP"
        par = NS()
        compiled = False
        @property
        def compileResult(self):
            self.compiled = True
            return "ERROR: shader failed"
        def errors(self):
            return "Compile failed" if self.compiled else ""
    out = build_inspect_node(Shader(), want_nodes=False, want_content=True, want_errors=True)
    assert out["content"]["compileState"] == "error"
    assert out["errors"] == ["Compile failed"]


def test_empty_compile_result_remains_unknown_through_inspect():
    shader = NS(path="/project1/shader", opType="glslTOP", family="TOP", par=NS(),
                compileResult="")
    out = build_inspect_node(shader, want_nodes=False, want_content=True)
    assert out["content"]["compileState"] == "unknown"


def test_shader_reference_failure_is_not_unbound_stage():
    shader = NS(path="/project1/shader", opType="glslTOP", family="TOP",
                par=NS(pixeldat=NS(eval=broken)), compileResult="")
    out = build_inspect_node(shader, want_nodes=False, want_content=True)
    stage = out["content"]["stages"][0]
    assert stage["role"] == "pixel"
    assert stage["evaluation"]["available"] is False


def test_detailed_menu_metadata_is_bounded_and_does_not_evaluate_twice():
    calls = []
    par = NS(name="attr0name", mode="CONSTANT", val="color", isMenu=True,
             menuNames=["color", "tex"] + [str(i) for i in range(40)],
             menuLabels=["Color", "Texture"] + [str(i) for i in range(40)])
    def evaluate():
        calls.append(True)
        return "color"
    par.eval = evaluate
    out = build_inspect_node(node(pars=lambda: [par]), want_nodes=False,
                             want_params=True, detail_level="detailed")
    item = out["params"][0]
    assert calls == [True]
    assert item["storedValue"] == item["val"] == "color"
    assert item["menu"]["names"][:2] == ["color", "tex"]
    assert len(item["menu"]["names"]) == 32
    assert item["menu"]["truncated"]


def test_menu_getter_failure_preserves_parameter_evaluation():
    class Par:
        name = "choice"
        mode = "CONSTANT"
        val = "custom"
        isMenu = True
        def eval(self):
            return self.val
        @property
        def menuNames(self):
            raise RuntimeError("dynamic menu unavailable")
    out = build_inspect_node(node(pars=lambda: [Par()]), want_nodes=False,
                             want_params=True, detail_level="detailed")
    item = out["params"][0]
    assert item["val"] == "custom"
    assert item["evaluation"]["available"]
    assert not item["menu"]["available"]

