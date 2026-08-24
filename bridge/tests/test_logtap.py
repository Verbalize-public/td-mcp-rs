"""Unit tests for the stdout/stderr log uplink tee (M2, no live TD)."""

from __future__ import annotations

import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import pytest  # noqa: E402

from tdmcp_bridge import logtap  # noqa: E402


@pytest.fixture(autouse=True)
def _reset(monkeypatch: pytest.MonkeyPatch):
    orig_stdout, orig_stderr = sys.stdout, sys.stderr
    logtap._reset_for_tests()
    yield
    logtap._reset_for_tests()
    sys.stdout, sys.stderr = orig_stdout, orig_stderr


def test_install_tees_stdout_and_stderr_write_through(capsys: pytest.CaptureFixture) -> None:
    logtap.install(lambda _records: None)
    print("hello")
    print("boom", file=sys.stderr)
    captured = capsys.readouterr()
    assert "hello" in captured.out
    assert "boom" in captured.err


def test_install_is_idempotent_does_not_double_wrap() -> None:
    logtap.install(lambda _records: None)
    first_tee = sys.stdout
    logtap.install(lambda _records: None)
    assert sys.stdout is first_tee


def test_install_wraps_whatever_is_current_even_after_reload_marker_reset() -> None:
    """Reinstall over a *different* Tee-like object (simulating a module
    reload rebuilding the Tee class) wraps it as the new "original" instead
    of losing write-through — duck-typed marker survives class identity churn."""
    logtap.install(lambda _records: None)
    stale_tee = sys.stdout

    class _NotOurTeeAnymore:
        _is_tdmcp_logtap_tee = False

        def __init__(self, inner):
            self.inner = inner
            self.writes: list[str] = []

        def write(self, s):
            self.writes.append(s)
            return len(s)

        def flush(self):
            pass

    fake_post_reload = _NotOurTeeAnymore(stale_tee)
    sys.stdout = fake_post_reload
    logtap.install(lambda _records: None)
    assert sys.stdout is not fake_post_reload
    sys.stdout.write("x")
    assert fake_post_reload.writes == ["x"], "must still write through the old handle"


def test_drop_oldest_marker_and_count() -> None:
    flushed: list[list[dict]] = []
    logtap.install(lambda records: flushed.append(records))
    for i in range(logtap._LOG_QUEUE_MAX + 5):
        logtap.append_local(f"line {i}")
    logtap.maybe_flush(force=True)
    assert len(flushed) == 1
    records = flushed[0]
    marker = [r for r in records if r["target"] == "bridge::logtap"]
    assert len(marker) == 1
    assert "dropped 5" in marker[0]["msg"]
    # Oldest 5 were evicted; buffer kept the newest _LOG_QUEUE_MAX lines.
    assert records[0]["msg"] == "line 5"


def test_maybe_flush_batches_by_line_count() -> None:
    flushed: list[list[dict]] = []
    logtap.install(lambda records: flushed.append(records))
    for i in range(logtap._BATCH_LINES - 1):
        logtap.append_local(f"line {i}")
    logtap.maybe_flush()
    assert flushed == [], "under batch size and under interval — must not flush yet"
    logtap.append_local("last")
    logtap.maybe_flush()
    assert len(flushed) == 1
    assert len(flushed[0]) == logtap._BATCH_LINES


def test_maybe_flush_batches_by_interval(monkeypatch: pytest.MonkeyPatch) -> None:
    flushed: list[list[dict]] = []
    logtap.install(lambda records: flushed.append(records))
    logtap.append_local("one")
    logtap.maybe_flush()
    assert flushed == [], "one line, no interval elapsed — must not flush yet"

    real_monotonic = time.monotonic
    monkeypatch.setattr(time, "monotonic", lambda: real_monotonic() + logtap._BATCH_INTERVAL_S + 1)
    logtap.maybe_flush()
    assert len(flushed) == 1
    assert flushed[0][0]["msg"] == "one"


def test_maybe_flush_noop_on_empty_buffer() -> None:
    calls = []
    logtap.install(lambda records: calls.append(records))
    logtap.maybe_flush(force=True)
    assert calls == []


def test_suppress_scopes_around_tee_writes() -> None:
    flushed: list[list[dict]] = []
    logtap.install(lambda records: flushed.append(records))
    with logtap.suppress():
        print("inside suppress")
    print("outside suppress")
    logtap.maybe_flush(force=True)
    assert len(flushed) == 1
    msgs = [r["msg"] for r in flushed[0]]
    assert "outside suppress" in msgs
    assert "inside suppress" not in msgs


def test_suppress_is_reentrant() -> None:
    flushed: list[list[dict]] = []
    logtap.install(lambda records: flushed.append(records))
    with logtap.suppress():
        with logtap.suppress():
            print("nested")
        print("still suppressed (outer still active)")
    print("now visible")
    logtap.maybe_flush(force=True)
    msgs = [r["msg"] for r in flushed[0]]
    assert msgs == ["now visible"]


def test_flush_failure_never_propagates() -> None:
    def boom(_records: list[dict]) -> None:
        raise RuntimeError("uplink down")

    logtap.install(boom)
    logtap.append_local("x")
    logtap.maybe_flush(force=True)  # must not raise


def test_append_local_does_not_touch_stdout(capsys: pytest.CaptureFixture) -> None:
    logtap.install(lambda _records: None)
    logtap.append_local("silent entry")
    captured = capsys.readouterr()
    assert captured.out == ""
    flushed: list[list[dict]] = []
    logtap._on_flush = flushed.append  # type: ignore[assignment]
    logtap.maybe_flush(force=True)
    assert flushed[0][0]["msg"] == "silent entry"
