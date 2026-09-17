"""Closed Menu behavior verified on TD 2025.32460; StrMenu is open-ended."""
import sys
from pathlib import Path
from types import SimpleNamespace as NS
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tdmcp_bridge.mutate import _apply_values


def make(**changes):
    values = dict(style="Menu", isCustom=False, menuSource=None, menuNames=["custom","color"], val="custom")
    values.update(changes)
    par = NS(**values)
    return NS(path="/project1/shader", opType="glslPOP", par=NS(attr0name=par)), par


@pytest.mark.parametrize("value", ["Color", "not-listed", -1, 999, 1.5, True, None])
def test_invalid_closed_choice_never_assigns(value):
    node, par = make()
    error = _apply_values(node, {"attr0name":value})
    assert error["code"] == "tdmcp.par.invalid_menu_value"
    assert error["field"] == "attr0name"
    assert error["allowedValues"] == ["custom","color"]
    assert par.val == "custom"


@pytest.mark.parametrize("value", ["color", 0, 1, 1.0])
def test_exact_token_and_valid_indices_remain_accepted(value):
    node, par = make()
    assert _apply_values(node, {"attr0name":value}) is None
    assert par.val == value


@pytest.mark.parametrize("changes", [{"style":"StrMenu"}, {"isCustom":True},
                                    {"menuSource":"dynamic"}, {"menuNames":[]},
                                    {"menuNames":[str(x) for x in range(257)]}])
def test_unverified_or_open_menu_is_not_rejected(changes):
    node, par = make(**changes)
    assert _apply_values(node, {"attr0name":"Unlisted"}) is None
    assert par.val == "Unlisted"


def test_earlier_fields_remain_applied_on_later_invalid_menu():
    node, par = make()
    node.par.gain = NS(val=1)
    error = _apply_values(node, {"gain":2,"attr0name":"Color"})
    assert error["code"] == "tdmcp.par.invalid_menu_value"
    assert node.par.gain.val == 2 and par.val == "custom"


def test_missing_menu_metadata_does_not_become_invalid_choice():
    node, par = make()
    del par.menuSource
    assert _apply_values(node, {"attr0name":"custom-value"}) is None
