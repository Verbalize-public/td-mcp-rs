"""Unit tests for execute_python structured exception reports."""

from __future__ import annotations

import tdmcp_bridge as bridge


def test_nested_raise_frames_and_type() -> None:
    script = (
        "def a():\n"
        "    return b()\n"
        "def b():\n"
        "    return c()\n"
        "def c():\n"
        "    raise RuntimeError('deep boom')\n"
        "result = a()\n"
    )
    out = bridge.handle_execute_python(
        {"script": script, "includeLogs": False, "formatMode": "normal"}
    )
    assert out["ok"] is False
    assert out["error"] == "deep boom"
    assert "RuntimeError" in (out.get("traceback") or "")
    exc = out["exception"]
    assert exc["type"] == "RuntimeError"
    assert exc["message"] == "deep boom"
    assert exc["raw"]
    assert exc["syntax"] is None
    names = [f["name"] for f in exc["frames"] if f["filename"] == "<string>"]
    assert names == ["<module>", "a", "b", "c"]
    assert all("locals" not in f for f in exc["frames"])


def test_syntax_error_block() -> None:
    out = bridge.handle_execute_python(
        {"script": "def broken(\n  pass", "includeLogs": False}
    )
    assert out["ok"] is False
    exc = out["exception"]
    assert exc["type"] == "SyntaxError"
    assert exc["syntax"] is not None
    assert exc["syntax"]["lineno"] == 1
    assert exc["syntax"]["msg"]


def test_debug_locals_only_on_string_frames() -> None:
    script = "x = 42\nraise ValueError('with locals')\n"
    normal = bridge.handle_execute_python(
        {"script": script, "includeLogs": False, "formatMode": "normal"}
    )
    debug = bridge.handle_execute_python(
        {"script": script, "includeLogs": False, "formatMode": "debug"}
    )
    assert normal["ok"] is False and debug["ok"] is False
    assert all("locals" not in f for f in normal["exception"]["frames"])
    string_frames = [
        f for f in debug["exception"]["frames"] if f["filename"] == "<string>"
    ]
    assert string_frames
    assert "locals" in string_frames[-1]
    locs = string_frames[-1]["locals"]
    assert "x" in locs
    assert locs["x"]["type"] == "int"
    assert "42" in locs["x"]["repr"]


def test_string_frame_line_filled_from_script() -> None:
    script = "a = 1\nraise RuntimeError('line check')\n"
    out = bridge.handle_execute_python(
        {"script": script, "includeLogs": False}
    )
    string_frames = [
        f for f in out["exception"]["frames"] if f["filename"] == "<string>"
    ]
    assert string_frames
    last = string_frames[-1]
    assert last["lineno"] == 2
    assert last["line"] is not None
    assert "RuntimeError" in last["line"]
