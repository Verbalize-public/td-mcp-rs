"""Unit tests for editor_context shaping (no live TD required)."""

from __future__ import annotations

import os
import sys
import unittest
from types import SimpleNamespace
from typing import Any
from unittest.mock import patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402


def _fake_op(path: str) -> SimpleNamespace:
    return SimpleNamespace(path=path, name=path.rsplit("/", 1)[-1])


def _fake_comp(
    path: str,
    *,
    selected: list[SimpleNamespace] | None = None,
    current: SimpleNamespace | None = None,
) -> SimpleNamespace:
    return SimpleNamespace(
        path=path,
        isCOMP=True,
        selectedChildren=list(selected or []),
        currentChild=current,
    )


def _fake_pane(
    *,
    pane_id: int,
    name: str,
    pane_type: str = "NETWORKEDITOR",
    owner: Any = None,
    blow_up: bool = False,
) -> Any:
    type_obj = SimpleNamespace(name=pane_type)
    if blow_up:

        class _BoomPane:
            def __init__(self) -> None:
                self.id = pane_id
                self.name = name
                self.type = type_obj

            @property
            def owner(self) -> Any:
                raise RuntimeError("boom")

        return _BoomPane()
    return SimpleNamespace(id=pane_id, name=name, type=type_obj, owner=owner)


class FakePanes(list):
    """List-like ui.panes stand-in with optional `.current`."""

    def __init__(self, items: list[Any], current: Any = None) -> None:
        super().__init__(items)
        self.current = current


class EditorContextTest(unittest.TestCase):
    def test_empty_when_ui_missing(self) -> None:
        fake_td = SimpleNamespace(ui=None)
        with patch.dict(sys.modules, {"td": fake_td}):
            result = tdmcp_bridge.handle_editor_context({})
        self.assertTrue(result["ok"])
        self.assertEqual(result["panes"], [])

    def test_no_focused_pane_all_unfocused(self) -> None:
        owner = _fake_comp("/project1")
        panes = FakePanes(
            [
                _fake_pane(pane_id=1, name="pane1", owner=owner),
                _fake_pane(pane_id=2, name="pane2", owner=owner),
            ],
            current=None,
        )
        fake_td = SimpleNamespace(ui=SimpleNamespace(panes=panes))
        with patch.dict(sys.modules, {"td": fake_td}):
            result = tdmcp_bridge.handle_editor_context({})
        self.assertTrue(result["ok"])
        self.assertEqual(len(result["panes"]), 2)
        self.assertFalse(result["panes"][0]["focused"])
        self.assertFalse(result["panes"][1]["focused"])
        self.assertNotIn("selection", result["panes"][0])

    def test_non_comp_owner_omits_selection(self) -> None:
        owner = SimpleNamespace(path="/project1/null1", isCOMP=False)
        pane = _fake_pane(pane_id=1, name="pane1", owner=owner)
        panes = FakePanes([pane], current=pane)
        fake_td = SimpleNamespace(ui=SimpleNamespace(panes=panes))
        with patch.dict(sys.modules, {"td": fake_td}):
            result = tdmcp_bridge.handle_editor_context({})
        entry = result["panes"][0]
        self.assertTrue(entry["focused"])
        self.assertEqual(entry["ownerPath"], "/project1/null1")
        self.assertNotIn("selection", entry)

    def test_selection_with_current_flag(self) -> None:
        a = _fake_op("/project1/base1/null1")
        b = _fake_op("/project1/base1/wave1")
        owner = _fake_comp("/project1/base1", selected=[a, b], current=a)
        pane = _fake_pane(pane_id=7, name="pane1", owner=owner)
        panes = FakePanes([pane], current=pane)
        fake_td = SimpleNamespace(ui=SimpleNamespace(panes=panes))
        with patch.dict(sys.modules, {"td": fake_td}):
            result = tdmcp_bridge.handle_editor_context({})
        sel = result["panes"][0]["selection"]
        self.assertEqual(len(sel), 2)
        self.assertEqual(sel[0], {"path": "/project1/base1/null1", "current": True})
        self.assertEqual(sel[1], {"path": "/project1/base1/wave1", "current": False})
        self.assertEqual(result["panes"][0]["type"], "NETWORKEDITOR")

    def test_selection_truncated(self) -> None:
        selected = [
            _fake_op(f"/project1/op{i}")
            for i in range(tdmcp_bridge.EDITOR_SELECTION_LIMIT + 5)
        ]
        owner = _fake_comp("/project1", selected=selected, current=selected[0])
        pane = _fake_pane(pane_id=1, name="pane1", owner=owner)
        panes = FakePanes([pane], current=pane)
        fake_td = SimpleNamespace(ui=SimpleNamespace(panes=panes))
        with patch.dict(sys.modules, {"td": fake_td}):
            result = tdmcp_bridge.handle_editor_context({})
        entry = result["panes"][0]
        self.assertTrue(entry["selectionTruncated"])
        self.assertEqual(len(entry["selection"]), tdmcp_bridge.EDITOR_SELECTION_LIMIT)
        self.assertEqual(entry["truncation"]["code"], "tdmcp.editor.selection_truncated")
        self.assertEqual(entry["truncation"]["limit"], tdmcp_bridge.EDITOR_SELECTION_LIMIT)

    def test_panes_truncated(self) -> None:
        items = [
            _fake_pane(pane_id=i, name=f"pane{i}", owner=_fake_comp(f"/project1/c{i}"))
            for i in range(tdmcp_bridge.EDITOR_PANES_LIMIT + 3)
        ]
        panes = FakePanes(items, current=items[0])
        fake_td = SimpleNamespace(ui=SimpleNamespace(panes=panes))
        with patch.dict(sys.modules, {"td": fake_td}):
            result = tdmcp_bridge.handle_editor_context({})
        self.assertTrue(result["panesTruncated"])
        self.assertEqual(len(result["panes"]), tdmcp_bridge.EDITOR_PANES_LIMIT)
        self.assertEqual(result["truncation"]["code"], "tdmcp.editor.panes_truncated")
        self.assertTrue(result["panes"][0]["focused"])

    def test_per_pane_failure_partial_success(self) -> None:
        good = _fake_pane(
            pane_id=1,
            name="good",
            owner=_fake_comp("/project1", selected=[_fake_op("/project1/a")]),
        )
        bad = _fake_pane(pane_id=2, name="bad", blow_up=True)
        panes = FakePanes([good, bad], current=good)
        fake_td = SimpleNamespace(ui=SimpleNamespace(panes=panes))
        with patch.dict(sys.modules, {"td": fake_td}):
            result = tdmcp_bridge.handle_editor_context({})
        self.assertTrue(result["ok"])
        self.assertTrue(result["panes"][0]["ok"])
        self.assertFalse(result["panes"][1]["ok"])
        self.assertEqual(result["panes"][1]["code"], "tdmcp.editor.pane_failed")
        self.assertIn("boom", result["panes"][1]["message"])

    def test_top_level_exception_coded(self) -> None:
        class _BadTd:
            @property
            def ui(self):  # noqa: ANN201
                raise RuntimeError("td.ui unavailable")

        with patch.dict(sys.modules, {"td": _BadTd()}):
            result = tdmcp_bridge.handle_editor_context({})
        self.assertFalse(result["ok"])
        self.assertEqual(result["code"], "tdmcp.editor.context_failed")
        self.assertIn("td.ui unavailable", result["message"])
        self.assertIn("traceback", result)


if __name__ == "__main__":
    unittest.main()
