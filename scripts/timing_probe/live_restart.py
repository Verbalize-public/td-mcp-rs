#!/usr/bin/env python3
"""Begin a job, then verify cleanup after an externally requested daemon restart.

Use only with the owned timing_probe fixture and a refreshed bootstrap.tox.
This script never restarts processes itself. Run begin, restart the daemon,
then verify with the same evidence path. No playback/UI recovery is allowed.
"""
import argparse
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mcp_probe import McpClient


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('phase', choices=('begin', 'verify'))
    parser.add_argument('--pid', type=int, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    client = McpClient()
    try:
        if args.phase == 'begin':
            baseline = client.call('execute_python', {'pid': args.pid, 'script': """
assert op('/project1/timing_probe') is not None
root.time.play = False
td.project.realTime = False
result = {'frame': float(root.time.frame)}
"""})['result']
            job = client.call('record', {'pid': args.pid,
                'path': '/project1/timing_probe/fx/out1', 'frames': 100})
            assert job['state'] == 'running', job
            evidence = {'pid': args.pid, 'baseline': baseline, 'start': job}
        else:
            evidence = json.loads(args.output.read_text())
            assert evidence['pid'] == args.pid
            started = time.monotonic()
            deadline = started + 45
            while True:
                fleet = client.call('fleet', {'pids': [args.pid]})
                if any(p['pid'] == args.pid and p['bridge'] == 'connected'
                       for p in fleet['processes']):
                    break
                if time.monotonic() > deadline:
                    raise TimeoutError('Paused/non-realtime bridge did not reconnect')
                time.sleep(.5)
            evidence['reconnectWaitSeconds'] = time.monotonic() - started
            old = client.request('tools/call', {'name': 'record', 'arguments': {
                'pid': args.pid, 'action': 'status', 'jobId': evidence['start']['jobId']}})
            assert old.get('isError'), old
            assert old['structuredContent']['items'][0]['code'] == 'tdmcp.timing.not_found', old
            evidence['oldJob'] = old
            result = client.call('execute_python', {'pid': args.pid, 'script': """
from tdmcp_bridge import timing
result = {'active': timing._active, 'play': bool(root.time.play),
          'realTime': bool(td.project.realTime),
          'recorders': [n.path for n in op('/project1/timing_probe/fx').children
                        if n.name.startswith('record_')],
          'runs': [r.group for r in td.runs if str(r.group).startswith('tdmcp-timing-')]}
"""})['result']
            evidence['cleanup'] = result
            assert result == {'active': None, 'play': False, 'realTime': False,
                              'recorders': [], 'runs': []}, result
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(evidence, indent=2) + '\n')
        print(json.dumps({'ok': True, 'phase': args.phase}))
    finally:
        client.close()


if __name__ == '__main__':
    main()
