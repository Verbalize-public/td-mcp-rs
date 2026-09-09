"""Off-thread TD-API guards — the "use from another Python thread" fix.

TD's Python API is main-thread-only. The global log tee fires the face
``./debug`` DAT mirror from whatever thread printed — including the IPC
worker writing its stream-teardown notes on daemon death / TD close /
project switch, and agent scripts' own threads. These tests pin the
defer-to-main-thread behavior: ``_debug_dat_mirror`` never touches a DAT
off the main thread, and ``drain_local`` (the td.run pump) writes parked
records from the main thread only.
"""

from __future__ import annotations

import os
import io
import sys
import threading
import types

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import pytest  # noqa: E402

import tdmcp_bridge  # noqa: E402
from tdmcp_bridge import logtap  # noqa: E402
from tdmcp_bridge import execute as execute_mod  # noqa: E402


@pytest.fixture(autouse=True)
def _reset_state():
    orig_stdout, orig_stderr = sys.stdout, sys.stderr
    orig_ident = tdmcp_bridge._main_thread_ident
    tdmcp_bridge._main_thread_ident = None
    with tdmcp_bridge._deferred_local_lock:
        tdmcp_bridge._deferred_local.clear()
    logtap._reset_for_tests()
    yield
    logtap._reset_for_tests()
    with tdmcp_bridge._deferred_local_lock:
        tdmcp_bridge._deferred_local.clear()
    tdmcp_bridge._main_thread_ident = orig_ident
    sys.stdout, sys.stderr = orig_stdout, orig_stderr


def _run_in_thread(fn, *args) -> None:
    t = threading.Thread(target=fn, args=args, daemon=True)
    t.start()
    t.join(timeout=5.0)
    assert not t.is_alive(), "worker thread hung"


def _deferred_msgs() -> list[str]:
    with tdmcp_bridge._deferred_local_lock:
        return [str(r.get("msg")) for r in tdmcp_bridge._deferred_local]


def test_is_main_thread_falls_back_to_python_main_thread() -> None:
    assert tdmcp_bridge.is_main_thread() is True
    box: list[bool] = []
    _run_in_thread(lambda: box.append(tdmcp_bridge.is_main_thread()))
    assert box == [False]


def test_is_main_thread_prefers_marked_ident() -> None:
    tdmcp_bridge.mark_main_thread()
    assert tdmcp_bridge.is_main_thread() is True
    box: list[bool] = []
    _run_in_thread(lambda: box.append(tdmcp_bridge.is_main_thread()))
    assert box == [False]


def test_mark_from_worker_thread_makes_main_thread_off_main() -> None:
    """A mark must never be taken from a non-main thread — if one ever is,
    the main thread itself must read as off-main (fail loud, not silently
    mirror off-thread)."""
    box: list[int] = []
    _run_in_thread(lambda: box.append(threading.get_ident()))
    tdmcp_bridge._main_thread_ident = box[0]
    assert tdmcp_bridge.is_main_thread() is False


def test_mirror_from_worker_thread_defers_instead_of_touching_dat(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    dat_calls: list[str] = []
    monkeypatch.setattr(
        tdmcp_bridge, "_append_debug_dat", lambda logs: dat_calls.append(logs)
    )
    record = {"level": "info", "target": "bridge::stdout", "msg": "from worker"}
    _run_in_thread(tdmcp_bridge._debug_dat_mirror, record)
    assert dat_calls == [], "worker thread must not write the debug DAT"
    assert _deferred_msgs() == ["from worker"]


def test_drain_local_writes_deferred_records_on_main_thread(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    dat_calls: list[str] = []
    monkeypatch.setattr(
        tdmcp_bridge, "_append_debug_dat", lambda logs: dat_calls.append(logs)
    )
    record = {"level": "error", "target": "bridge::stderr", "msg": "stream closed"}
    _run_in_thread(tdmcp_bridge._debug_dat_mirror, record)
    assert tdmcp_bridge.drain_local() == 1
    assert len(dat_calls) == 1
    assert "stream closed" in dat_calls[0]
    assert tdmcp_bridge.drain_local() == 0  # drained — idempotent
    assert _deferred_msgs() == []


def test_drain_local_noop_off_main_thread() -> None:
    tdmcp_bridge.mark_main_thread()
    tdmcp_bridge._defer_local_record({"level": "info", "target": "t", "msg": "x"})
    results: list[int] = []
    _run_in_thread(lambda: results.append(tdmcp_bridge.drain_local()))
    assert results == [0], "off-main drain must be a no-op"
    assert _deferred_msgs() == ["x"], "record must stay parked for the pump"
    assert tdmcp_bridge.drain_local() == 1  # main thread flushes it


def test_deferred_queue_drops_oldest_beyond_cap() -> None:
    for i in range(tdmcp_bridge._DEFERRED_LOCAL_MAX + 5):
        tdmcp_bridge._defer_local_record(
            {"level": "info", "target": "t", "msg": str(i)}
        )
    msgs = _deferred_msgs()
    assert len(msgs) == tdmcp_bridge._DEFERRED_LOCAL_MAX
    assert msgs[0] == "5", "oldest records must be evicted, drop-oldest"


def test_worker_thread_stderr_write_via_tee_defers_mirror(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """End-to-end for the serve_queued teardown note: the IPC worker writes
    its "stream closed" note to sys.stderr (the installed tee) — that must
    reach the textport and the uplink buffer but never the debug DAT until
    the main thread drains."""
    dat_calls: list[str] = []
    monkeypatch.setattr(
        tdmcp_bridge, "_append_debug_dat", lambda logs: dat_calls.append(logs)
    )
    logtap.install(
        tdmcp_bridge._bridge_log_sender, on_local=tdmcp_bridge._debug_dat_mirror
    )
    _run_in_thread(
        sys.stderr.write, "tdmcp_bridge: serve_queued stopping (stream closed)\n"
    )
    assert dat_calls == [], "worker-thread print must not write the debug DAT"
    assert any("stream closed" in m for m in _deferred_msgs())
    assert tdmcp_bridge.drain_local() >= 1
    assert any("stream closed" in c for c in dat_calls)


def test_mirror_on_main_thread_writes_directly(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Main-thread mirror calls stay synchronous (face LOGS freshness)."""
    dat_calls: list[str] = []
    monkeypatch.setattr(
        tdmcp_bridge, "_append_debug_dat", lambda logs: dat_calls.append(logs)
    )
    tdmcp_bridge._debug_dat_mirror(
        {"level": "info", "target": "t", "msg": "direct"}
    )
    assert len(dat_calls) == 1
    assert _deferred_msgs() == []


class _MainThreadStream:
    """Model TD's native Textport: even attribute access can enter TD/UI code."""

    def __init__(self):
        self.owner_ident = threading.get_ident()
        self.calls = []
        self.text = []

    def _record(self, operation):
        self.calls.append((operation, threading.get_ident()))

    def write(self, text):
        self._record('write')
        self.text.append(text)
        return len(text)

    def flush(self):
        self._record('flush')

    def isatty(self):
        self._record('isatty')
        return True

    @property
    def encoding(self):
        self._record('encoding')
        return 'utf-8'


@pytest.mark.parametrize('capture', [False, True])
def test_worker_output_never_enters_native_textport(capture):
    native = _MainThreadStream()
    stream = logtap.Tee(native, 'error', 'bridge::stderr')
    buf = io.StringIO()
    if capture:
        stream = execute_mod._TeeStream(buf, stream)

    def worker():
        stream.write('worker traceback\n')
        stream.flush()
        stream.isatty()
        _ = stream.encoding

    _run_in_thread(worker)
    assert native.calls == [], 'no native stream access is safe on the worker'
    _run_in_thread(logtap.drain_streams)
    assert native.calls == [], 'off-main drain must not enter Textport either'
    logtap.drain_streams()
    assert ''.join(native.text) == 'worker traceback\n'
    assert all(ident == native.owner_ident for _, ident in native.calls)
    if capture:
        assert buf.getvalue() == 'worker traceback\n'


def test_execute_worker_print_during_capture_is_not_uplinked_twice(monkeypatch):
    native = _MainThreadStream()
    monkeypatch.setattr(sys, 'stdout', native)
    monkeypatch.setattr(sys, 'stderr', native)
    logtap.install(lambda records: None)
    result = tdmcp_bridge.handle_execute_python({
        'script': "import threading\nt = threading.Thread(target=lambda: print('worker output'))\nt.start()\nt.join(2)\nresult = not t.is_alive()"
    })
    assert result['result'] is True
    assert 'worker output' in result['logs']
    assert all(ident == native.owner_ident for _, ident in native.calls)
    logtap.drain_streams()
    assert ''.join(native.text) == 'worker output\n'
    assert len(logtap._buffer) == 1
    assert logtap._buffer[0]['target'] == 'execute_python'


def test_capture_without_global_tee_defers_raw_native_stream():
    native = _MainThreadStream()
    stream = execute_mod._TeeStream(io.StringIO(), native)
    _run_in_thread(stream.write, 'raw native stream')
    assert native.calls == []
    logtap.drain_streams()
    assert native.text == ['raw native stream']


def test_worker_teardown_does_not_touch_textport(monkeypatch):
    from tdmcp_bridge import task_queue

    native = _MainThreadStream()
    monkeypatch.setattr(sys, 'stderr', native)
    logtap.install(lambda records: None, on_local=tdmcp_bridge._debug_dat_mirror)

    def broken_read(*args, **kwargs):
        raise OSError('connection closed')

    monkeypatch.setattr(task_queue, '_read_frame', broken_read)
    stream = types.SimpleNamespace(close=lambda: None)
    _run_in_thread(task_queue.serve_queued, stream)
    assert native.calls == []
    assert any('stream closed' in msg for msg in _deferred_msgs())
    logtap.drain_streams()
    assert 'stream closed' in ''.join(native.text)
    assert all(ident == native.owner_ident for _, ident in native.calls)


def test_deferred_native_output_is_bounded_and_preserves_surviving_order():
    native = _MainThreadStream()

    def worker():
        for i in range(logtap._STREAM_QUEUE_MAX + 4):
            logtap.write_stream(native, f'{i}:')
        logtap.write_stream(native, 'x' * (logtap._STREAM_CHUNK_MAX + 100))

    _run_in_thread(worker)
    assert native.calls == []
    assert logtap.drain_streams() == logtap._STREAM_QUEUE_MAX
    assert native.text[0] == '5:'
    assert native.text[-1] == 'x' * logtap._STREAM_CHUNK_MAX
    assert native.text[1:-1] == [f'{i}:' for i in range(6, logtap._STREAM_QUEUE_MAX + 4)]


@pytest.mark.parametrize('entry', ['dispatch', 'execute', 'pump', 'bootstrap'])
def test_td_entry_points_reject_worker_before_any_handler_or_reload(monkeypatch, entry):
    calls = []
    monkeypatch.setitem(tdmcp_bridge.HANDLERS, 'inspect', lambda params: calls.append(params))
    monkeypatch.setattr(tdmcp_bridge, '_load_bridge_package', lambda path: calls.append(path))
    entries = {
        'dispatch': lambda: tdmcp_bridge.dispatch({'type': 'request', 'method': 'inspect'}),
        'execute': lambda: tdmcp_bridge.handle_execute_python({'script': 'result = 1'}),
        'pump': tdmcp_bridge.process_pending,
        'bootstrap': tdmcp_bridge.bootstrap_threaded,
    }
    errors = []

    def worker():
        try:
            entries[entry]()
        except RuntimeError as exc:
            errors.append(str(exc))

    _run_in_thread(worker)
    assert len(errors) == 1 and 'requires the main thread' in errors[0]
    assert calls == []
    assert tdmcp_bridge._main_thread_ident is None
