"""Paused/non-realtime reconnection uses elapsed wall time, not frame count."""
import sys
from pathlib import Path
from types import SimpleNamespace


def test_reconnect_delay_uses_wall_time(monkeypatch):
    source = Path(__file__).resolve().parents[1] / 'tox_callbacks.py'
    namespace = {'__name__': 'test_callbacks'}
    exec(compile(source.read_text(), str(source), 'exec'), namespace)
    scheduled = []
    ref = object()
    monkeypatch.setitem(sys.modules, 'td', SimpleNamespace(run=lambda script, **kwargs: scheduled.append(kwargs)))
    namespace.update({'me': SimpleNamespace(path='/project1/tdmcp_rs/tdmcp_exec'),
                      '_comp': lambda: object(), '_par_bool': lambda *a: True,
                      '_bridge_mod': lambda: None, '_bridge_connected': lambda m: False,
                      '_run_bootstrap': lambda: None, '_td_delay_ref': lambda: ref})
    namespace['_reconnect_watchdog']()
    assert len(scheduled) == 1
    assert scheduled[0]['delayRef'] is ref
    assert scheduled[0]['delayMilliSeconds'] == 2000
    assert scheduled[0].get('wallTime') is True
