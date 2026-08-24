"""Unit tests for execute_python stdout/stderr capture (no live TD)."""

from __future__ import annotations

import io
import sys

import pytest

import tdmcp_bridge as bridge
from tdmcp_bridge import execute as _execute_mod
from tdmcp_bridge import logtap


@pytest.fixture(autouse=True)
def _reset_capture_state(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(bridge, "_capture_depth", 0)
    monkeypatch.setattr(bridge, "_bridge_host_path", None)
    monkeypatch.setattr(bridge, "_append_debug_dat", lambda _logs: None)
    logtap._reset_for_tests()
    yield
    logtap._reset_for_tests()


def test_truncate_logs_under_limit() -> None:
    assert bridge._truncate_logs("hi", limit=100) == "hi"


def test_truncate_logs_over_limit() -> None:
    text = "x" * 100
    out = bridge._truncate_logs(text, limit=40)
    assert out.startswith(bridge._TRUNC_MARK)
    assert len(out) <= 40
    assert out.endswith("x" * 10) or "x" in out


def test_ring_append_keeps_tail() -> None:
    existing = "a" * 50
    chunk = "b" * 50
    merged = bridge._ring_append_text(existing, chunk, limit=60)
    assert len(merged) == 60
    assert merged.endswith("b" * 50)
    assert merged.startswith("a" * 10)


def test_capture_print_restores_streams() -> None:
    sink = io.StringIO()
    old_out, old_err = sys.stdout, sys.stderr
    sys.stdout = sink
    sys.stderr = sink
    try:
        out = bridge.handle_execute_python(
            {"script": "print('hello-log')\nresult = 7", "includeLogs": True}
        )
        assert out["ok"] is True
        assert out["result"] == 7
        assert "hello-log" in out["logs"]
        assert "hello-log" in sink.getvalue()
        assert sys.stdout is sink
        assert sys.stderr is sink
    finally:
        sys.stdout, sys.stderr = old_out, old_err


def test_capture_restores_after_exception() -> None:
    sink = io.StringIO()
    old_out, old_err = sys.stdout, sys.stderr
    sys.stdout = sink
    sys.stderr = sink
    try:
        out = bridge.handle_execute_python(
            {
                "script": "print('before-boom')\nraise ValueError('boom')",
                "includeLogs": True,
            }
        )
        assert out["ok"] is False
        assert "before-boom" in out["logs"]
        assert "boom" in (out.get("error") or "")
        assert "ValueError" in (out.get("traceback") or "")
        assert sys.stdout is sink
        assert sys.stderr is sink
    finally:
        sys.stdout, sys.stderr = old_out, old_err


def test_include_logs_false_leaves_streams_and_omits_logs() -> None:
    sink = io.StringIO()
    old_out, old_err = sys.stdout, sys.stderr
    sys.stdout = sink
    sys.stderr = sink
    try:
        marker = sys.stdout
        out = bridge.handle_execute_python(
            {"script": "print('secret')\nresult = 1", "includeLogs": False}
        )
        assert out["ok"] is True
        assert "logs" not in out
        assert sys.stdout is marker
        assert "secret" in sink.getvalue()
    finally:
        sys.stdout, sys.stderr = old_out, old_err


def test_nested_capture_restores_once() -> None:
    sink = io.StringIO()
    old_out, old_err = sys.stdout, sys.stderr
    sys.stdout = sink
    sys.stderr = sink
    try:
        script = (
            "import tdmcp_bridge as b\n"
            "inner = b.handle_execute_python("
            "{'script': \"print('inner')\\nresult = 2\", 'includeLogs': True})\n"
            "print('outer')\n"
            "result = inner['result']\n"
        )
        out = bridge.handle_execute_python({"script": script, "includeLogs": True})
        assert out["ok"] is True
        assert out["result"] == 2
        assert "outer" in out["logs"]
        assert "inner" in out["logs"]
        assert sys.stdout is sink
        assert bridge._capture_depth == 0
    finally:
        sys.stdout, sys.stderr = old_out, old_err


def test_tee_coerces_non_str() -> None:
    buf = io.StringIO()
    prev = io.StringIO()
    tee = bridge._TeeStream(buf, prev)
    tee.write(123)
    assert buf.getvalue() == "123"
    assert prev.getvalue() == "123"


def test_execute_python_exposes_td_and_op_globals(monkeypatch: pytest.MonkeyPatch) -> None:
    import types

    fake_td = types.SimpleNamespace(op=lambda path: f"resolved:{path}", noiseTOP=object())
    monkeypatch.setitem(sys.modules, "td", fake_td)
    out = bridge.handle_execute_python(
        {
            "script": (
                "assert td is not None\n"
                "assert callable(op)\n"
                "result = op('/project1')\n"
            ),
            "includeLogs": False,
        }
    )
    assert out["ok"] is True
    assert out["result"] == "resolved:/project1"


def test_execute_python_exposes_closed_set_aliases(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import types

    fake_root = object()
    fake_ui = object()
    fake_td = types.SimpleNamespace(
        op=lambda _p: None,
        root=fake_root,
        ui=fake_ui,
        project=object(),
        absTime=object(),
        tdu=object(),
        run=lambda *_a, **_k: None,
        ops=object(),
        # opex / passive / mod intentionally missing — must not be invented
    )
    monkeypatch.setitem(sys.modules, "td", fake_td)
    out = bridge.handle_execute_python(
        {
            "script": (
                "assert root is td.root\n"
                "assert ui is td.ui\n"
                "assert project is td.project\n"
                "assert absTime is td.absTime\n"
                "assert tdu is td.tdu\n"
                "assert run is td.run\n"
                "assert ops is td.ops\n"
                "assert 'opex' not in globals()\n"
                "assert 'passive' not in globals()\n"
                "assert 'mod' not in globals()\n"
                "assert 'me' not in globals()\n"
                "assert 'parent' not in globals()\n"
                "assert 'noiseTOP' not in globals()\n"
                "assert 'debug' not in globals()\n"
                "result = 'ok'\n"
            ),
            "includeLogs": False,
        }
    )
    assert out["ok"] is True
    assert out["result"] == "ok"


def test_execute_python_keeps_tdmcp_resolve(monkeypatch: pytest.MonkeyPatch) -> None:
    import types

    monkeypatch.setitem(sys.modules, "td", types.SimpleNamespace(op=lambda _p: None))
    out = bridge.handle_execute_python(
        {
            "script": (
                "assert callable(tdmcp_resolve)\n"
                "assert __tdmcp_context_path__ == '/project1'\n"
                "result = 'ok'\n"
            ),
            "contextPath": "/project1",
            "includeLogs": False,
        }
    )
    assert out["ok"] is True
    assert out["result"] == "ok"


def test_execute_python_rejects_oversized_script() -> None:
    script = "x" * (bridge.SCRIPT_MAX_BYTES + 1)
    out = bridge.handle_execute_python({"script": script, "includeLogs": False})
    assert out["ok"] is False
    assert out["code"] == "tdmcp.script.too_large"
    assert "exceeds" in (out.get("error") or "")


def test_tap_installed_routes_single_combined_record_not_per_line(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """M3 T3.2: with the uplink tap installed, a script printing multiple
    lines must uplink as ONE record (the whole block via append_local) —
    not one record per print, which double-counts every line the tool's own
    `logs` field already returns."""
    sink = io.StringIO()
    old_out, old_err = sys.stdout, sys.stderr
    dat_calls: list[str] = []
    monkeypatch.setattr(_execute_mod, "_append_debug_dat", lambda logs: dat_calls.append(logs))
    logtap.install(lambda _records: None)
    # install() swapped sys.stdout to a fresh tee wrapping whatever was
    # there — point that "whatever" at the test sink first.
    logtap._orig_stdout = sink
    sys.stdout = logtap.Tee(sink, "info", "bridge::stdout")
    logtap._orig_stderr = sink
    sys.stderr = logtap.Tee(sink, "error", "bridge::stderr")
    try:
        out = bridge.handle_execute_python(
            {"script": "print('a')\nprint('b')\nprint('c')\nresult = 1", "includeLogs": True}
        )
        assert out["ok"] is True
        assert "a" in out["logs"] and "b" in out["logs"] and "c" in out["logs"]
        assert len(logtap._buffer) == 1, "must be one combined record, not one per print"
        assert logtap._buffer[0]["target"] == "execute_python"
        assert dat_calls == [], "must not also use the legacy direct-DAT path"
    finally:
        sys.stdout, sys.stderr = old_out, old_err


def test_tap_not_installed_falls_back_to_legacy_dat_write(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    sink = io.StringIO()
    old_out, old_err = sys.stdout, sys.stderr
    sys.stdout = sink
    sys.stderr = sink
    dat_calls: list[str] = []
    monkeypatch.setattr(_execute_mod, "_append_debug_dat", lambda logs: dat_calls.append(logs))
    assert not logtap.is_installed()
    try:
        out = bridge.handle_execute_python(
            {"script": "print('legacy-path')\nresult = 1", "includeLogs": True}
        )
        assert out["ok"] is True
        assert dat_calls and "legacy-path" in dat_calls[0]
    finally:
        sys.stdout, sys.stderr = old_out, old_err


def test_execute_python_rejects_oversized_result() -> None:
    # Tiny script builds a large runtime result (must not trip SCRIPT_MAX_BYTES).
    out = bridge.handle_execute_python(
        {
            "script": f"result = 'x' * {bridge.RESULT_MAX_BYTES + 64}",
            "includeLogs": False,
        }
    )
    assert out["ok"] is False
    assert out["code"] == "tdmcp.script.result_too_large"
    assert out.get("result") is None
