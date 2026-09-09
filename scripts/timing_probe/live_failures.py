#!/usr/bin/env python3
"""Main-thread failure injection on the owned timing_probe scratch network."""
import argparse
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mcp_probe import McpClient


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', required=True, type=int)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    client = McpClient()
    evidence = {}
    try:
        for cause in ('cancel', 'source', 'seek', 'play', 'encoder', 'timeout'):
            script = """
import tdmcp_bridge.timing as timing
b = op('/project1/timing_probe')
if b is None:
    raise RuntimeError('Owned scratch fixture is missing')
root.time.play = False
project.realTime = False
source = b.op('failure_source') or b.create(td.constantTOP, 'failure_source')
source.par.resolutionw = source.par.resolutionh = 64
source.comment = 'Owned temporary source for timing failure acceptance'
cause = CAUSE
kind = 'record' if cause in ('cancel', 'encoder') else 'capture'
params = {'path':source.path, 'frames':20, 'timing':{'initializeFrames':0}}
if kind == 'capture':
    params['timing']['sampleFrames'] = [0, 10]
if cause == 'timeout':
    params['timing']['timeoutSeconds'] = 1
    params['timing']['sampleFrames'] = [0, 120]
started = timing.handle(kind, params)
if not started.get('jobId'):
    raise RuntimeError(started)
job = timing._jobs[started['jobId']]
# Injection is scheduled here on MAIN, not from a worker thread.
def inject():
    if cause == 'cancel':
        timing.handle(kind, {'action':'cancel', 'jobId':job.id})
    elif cause == 'source':
        source.destroy()
    elif cause == 'seek':
        root.time.frame += 2
    elif cause == 'play':
        root.time.play = True
    elif cause == 'encoder' and job.movie is not None:
        job.movie.setInputs([])
if cause != 'timeout':
    td.run(inject, delayFrames=8, endFrame=True, delayRef=op.TDResources, group='tdmcp-failure-inject')
blocked = timing.admission('execute_python', {})
result = {'kind':kind, 'jobId':job.id, 'blocked':blocked}
""".replace('CAUSE', repr(cause))
            start = client.call('execute_python', {'pid': args.pid, 'script': script})['result']
            assert start['blocked']['code'] == 'tdmcp.timing.busy'
            deadline = time.monotonic() + 160
            while True:
                job = client.call(start['kind'], {'pid': args.pid, 'action': 'status', 'jobId': start['jobId']})
                if job['state'] not in ('running', 'finalizing'):
                    break
                if time.monotonic() >= deadline:
                    raise TimeoutError(cause)
                time.sleep(.1)
            evidence[cause] = job
            assert job['state'] == ('cancelled' if cause == 'cancel' else 'failed'), job
            assert job['advancedFrames'] < (120 if cause == 'timeout' else 20)
            after = client.call('execute_python', {'pid': args.pid, 'script': "import tdmcp_bridge.timing as timing\nresult={'play':bool(root.time.play),'active':timing._active,'runs':[r.group for r in td.runs if r.group.startswith('tdmcp-timing-')]}"})['result']
            assert after == {'play': False, 'active': None, 'runs': []}, after
            evidence[cause]['afterCleanup'] = after
            print(cause, job['state'], job['advancedFrames'], flush=True)
    finally:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(evidence, indent=2) + '\n')
        client.close()


if __name__ == '__main__':
    main()
