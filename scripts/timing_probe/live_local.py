#!/usr/bin/env python3
"""Verify independent local-time capture and recording on an owned fixture."""
import argparse
import json
from pathlib import Path
import sys
import time
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mcp_probe import McpClient


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', type=int, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    client = McpClient()
    evidence = {}
    try:
        setup = """
b = op('/project1/timing_probe')
if b is None: raise RuntimeError('Owned fixture required')
c = b.op('local_probe') or b.create(td.baseCOMP, 'local_probe')
l = c.op('local') or c.create(td.baseCOMP, 'local')
t = l.op('time') or l.copy(op('/local/time'), name='time')
t.par.independent = True
t.play = False
s = c.op('source') or c.create(td.constantTOP, 'source')
s.par.resolutionw = s.par.resolutionh = 64
s.par.colorr.expr = 'me.time.frame / 600'
result = {'path':s.path,'timePath':s.time.path,'frame':float(s.time.frame),'rootFrame':float(root.time.frame)}
"""
        initial = client.call('execute_python', {'pid':args.pid, 'script':setup})['result']
        assert initial['timePath'].startswith('/project1/timing_probe/local_probe/')
        evidence['initial'] = initial
        for tool in ('capture', 'record'):
            params = {'pid':args.pid, 'path':initial['path'], 'timing':{'timePath':initial['timePath']}}
            if tool == 'capture':
                params['timing']['sampleFrames'] = [0, 1, 3]
                params['maxSize'] = 64
            else:
                params['frames'] = 3
            result = client.call(tool, params)
            deadline = time.monotonic() + 160
            while result['state'] in ('running', 'finalizing'):
                if time.monotonic() > deadline: raise TimeoutError(tool)
                time.sleep(.1)
                result = client.call(tool, {'pid':args.pid, 'action':'status', 'jobId':result['jobId']})
            evidence[tool] = result
            assert result['state'] == 'complete', result
            if tool == 'capture':
                assert [s['timing']['frame'] - initial['frame'] for s in result['samples']] == [0, 1, 3]
            else:
                assert result['artifact']['frames'] == 3
        final = client.call('execute_python', {'pid':args.pid, 'script':"result={'rootFrame':float(root.time.frame),'play':bool(root.time.play)}"})['result']
        evidence['final'] = final
        assert final['rootFrame'] == initial['rootFrame'] and not final['play']
        print(json.dumps({'ok':True, 'rootUnchanged':True,'localCapture':True,'localRecordFrames':3}))
    finally:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(evidence, indent=2) + '\n')
        client.close()


if __name__ == '__main__':
    main()
