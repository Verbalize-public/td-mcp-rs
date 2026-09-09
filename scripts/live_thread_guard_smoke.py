#!/usr/bin/env python3
"""Exercise worker logging through real MCP on an owned scratch TD process.

Usage: python3 scripts/live_thread_guard_smoke.py PID --output evidence.json
Optional --pause tests bridge responsiveness with a main-thread recovery timer.
Workers only use Python streams and bridge guards, never TD objects or UI.
"""
import argparse
import json
from pathlib import Path
import time

from mcp_probe import McpClient


WORKER_PROBE = """
import sys
import threading
import tdmcp_bridge as bridge
from tdmcp_bridge import logtap

if not bridge.is_main_thread():
    raise RuntimeError('execute_python itself is off-main')
checks = []
def worker():
    checks.append({'workerIsMain': bridge.is_main_thread()})
    for stream in (sys.stdout, sys.stderr):
        stream.write('tdmcp thread-guard smoke: worker output\\n')
        stream.flush()
        checks.append({'isatty': stream.isatty()})
    try:
        bridge.process_pending()
    except RuntimeError as exc:
        checks.append({'rejected': str(exc)})
thread = threading.Thread(target=worker, daemon=True)
thread.start()
thread.join(timeout=1)
result = {'mainThread': bridge.is_main_thread(), 'workerFinished': not thread.is_alive(),
          'checks': checks, 'pendingTextport': len(logtap._deferred_streams),
          'guardLoaded': hasattr(bridge, 'require_main_thread'),
          'bridgeFile': bridge.__file__}
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('pid', type=int)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--pause', action='store_true')
    parser.add_argument('--non-realtime', action='store_true')
    args = parser.parse_args()
    if args.non_realtime and not args.pause:
        parser.error('--non-realtime requires --pause')
    client = McpClient()
    evidence = {'pid': args.pid}
    original = None
    try:
        fleet = client.call('fleet', {'pids': [args.pid]})
        if not any(p.get('bridge') == 'connected' for p in fleet['processes']):
            raise RuntimeError('PID must be connected and owned by the tester')
        for capture in (False, True):
            answer = client.call('execute_python', {
                'pid': args.pid, 'script': WORKER_PROBE, 'includeLogs': capture,
            })
            evidence[f'capture_{capture}'] = answer
            checked = answer['result']
            assert checked['mainThread'] and checked['workerFinished'] and checked['guardLoaded']
            assert checked['pendingTextport'] >= 4
            assert any('requires the main thread' in c.get('rejected', '') for c in checked['checks'])
        evidence['drainPolls'] = []
        for _ in range(3):
            time.sleep(0.1)  # Wait for the main-thread pump, not timeline stepping.
            drained = client.call('execute_python', {
                'pid': args.pid, 'includeLogs': False,
                'script': "from tdmcp_bridge import logtap\nresult = {'pendingTextport':len(logtap._deferred_streams), 'play':bool(root.time.play)}",
            })
            evidence['drainPolls'].append(drained)
            if drained['result']['pendingTextport'] == 0:
                break
        assert drained['result']['pendingTextport'] == 0
        if args.pause:
            original = client.call('execute_python', {
                'pid': args.pid, 'script': "result = {'play': bool(root.time.play), 'realTime': bool(project.realTime)}",
            })['result']
            evidence['beforePause'] = original
            evidence['pause'] = client.call('execute_python', {
                'pid': args.pid,
                'script': f"td.run(lambda: (setattr(project, 'realTime', {original['realTime']!r}), setattr(root.time, 'play', {original['play']!r})), delayMilliSeconds=3000, wallTime=True, delayRef=op.TDResources, group='tdmcp-thread-guard-recovery')\nproject.realTime = {False if args.non_realtime else original['realTime']!r}\nroot.time.play = False\nresult = {{'play':bool(root.time.play), 'frame':float(root.time.frame), 'realTime':bool(project.realTime)}}",
            })
            evidence['pausedInspect'] = client.call('inspect', {'pid': args.pid, 'paths': ['/local/time']})
            evidence['afterPausedInspect'] = client.call('execute_python', {
                'pid': args.pid, 'script': "result = {'play': bool(root.time.play), 'frame': float(root.time.frame), 'realTime':bool(project.realTime)}",
            })
            assert evidence['afterPausedInspect']['result']['play'] is False
            if args.non_realtime:
                assert evidence['afterPausedInspect']['result']['realTime'] is False
    finally:
        try:
            if original is not None:
                evidence['restore'] = client.call('execute_python', {
                    'pid': args.pid,
                    'script': f"project.realTime = {original['realTime']!r}\nroot.time.play = {original['play']!r}\nfor task in list(td.runs):\n    if task.group == 'tdmcp-thread-guard-recovery':\n        task.kill()\nresult = {{'play':bool(root.time.play),'realTime':bool(project.realTime)}}",
                })
        finally:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(evidence, indent=2) + '\n')
            client.close()
    print(json.dumps(evidence, indent=2))


if __name__ == '__main__':
    main()
