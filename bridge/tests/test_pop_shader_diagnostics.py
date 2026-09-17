"""Info DAT logs observed on live TD 2025.32460, including dirty-DAT refresh."""
import sys
from pathlib import Path
from types import SimpleNamespace as NS
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tdmcp_bridge.shader_lint import observe_compile_result

SUCCESS = "\n=============\nCompute Shader Compile Results:\n\nCompiled Successfully\n\n==========\n\n"
FAILURE = "Compute Shader Compile Results:\nERROR: /project1/code:1: undeclared identifier\nERROR: 1 compilation errors. No code generated.\n"


def par(value):
    return NS(eval=lambda: value)


def fixture(text=SUCCESS):
    shader = NS(path="/project1/shader", errors=lambda: "")
    info = NS(path="/project1/shader_info", opType="infoDAT", valid=True, text=text,
              par=NS(op=par(shader), infotype=par("general"), passive=par(False)))
    shader.docked = [info]
    shader.parent = lambda: NS(children=[info])
    return shader, info


@pytest.mark.parametrize("log,code", [(SUCCESS,"tdmcp.shader.compiled"),
                                    (FAILURE,"tdmcp.shader.compile_failed"),
                                    ("","tdmcp.shader.state_unknown")])
def test_bound_info_dat_is_classified_with_provenance(log, code):
    shader, info = fixture(log)
    result = observe_compile_result(shader, "glslPOP")
    assert result["code"] == code
    assert result["source"] == "infoDAT" and result["infoDatPath"] == info.path
    assert result["log"] == log
    if code.endswith("compile_failed"):
        assert "/project1/code:1:" in result["lines"][0]


@pytest.mark.parametrize("change", ["wrong_target", "passive", "wrong_mode", "wrong_type", "invalid"])
def test_unverified_info_surface_cannot_establish_compile_success(change):
    shader, info = fixture()
    if change == "wrong_target":
        info.par.op = par(NS(path="/project1/other"))
    elif change == "passive":
        info.par.passive = par(True)
    elif change == "wrong_mode":
        info.par.infotype = par("other")
    elif change == "wrong_type":
        info.opType = "textDAT"
    else:
        info.valid = False
    assert observe_compile_result(shader, "glslPOP")["code"] == "tdmcp.shader.unsupported_consumer"


def test_sibling_info_dat_can_be_used_without_guessing_its_name():
    shader, info = fixture()
    shader.docked = []
    info.path = "/project1/my_compile_observer"
    assert observe_compile_result(shader, "glslPOP")["infoDatPath"] == info.path


def test_info_dat_read_failure_is_unknown_not_success():
    shader, info = fixture()
    class Broken:
        path = info.path
        opType = "infoDAT"
        valid = True
        par = info.par
        @property
        def text(self):
            raise RuntimeError("unavailable")
    shader.docked = [Broken()]
    result = observe_compile_result(shader, "glslPOP")
    assert result["code"] == "tdmcp.shader.state_unknown"
    assert result["readError"]["type"] == "RuntimeError"


def test_info_search_is_bounded_and_does_not_create_nodes():
    shader, info = fixture()
    shader.docked = [NS(path=str(i), opType="nullTOP", valid=True) for i in range(64)] + [info]
    shader.parent = lambda: NS(children=[])
    result = observe_compile_result(shader, "glslPOP")
    assert result["code"] == "tdmcp.shader.unsupported_consumer"
