#!/usr/bin/env python3
"""Verify the timing reservation across two real MCP sessions, sequential calls."""
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
    a, b = McpClient(), McpClient()
    evidence = {}
    try:
        r = a.call('capture', {'pid':args.pid, 'path':'/project1/timing_probe/fx/out1',
                              'maxSize':64, 'timing':{'sampleFrames':[0,100]}})
        evidence['start'] = r
        blocked = b.request('tools/call', {'name':'inspect','arguments':{'pid':args.pid,'paths':['/project1']}})
        evidence['blocked'] = blocked
        assert blocked['isError']
        assert blocked['structuredContent']['items'][0]['code'] == 'tdmcp.timing.busy'
        # Discover from another session after pretending the start reply was lost.
        recovered = b.call('capture', {'pid':args.pid,'action':'status'})
        assert recovered['jobId'] == r['jobId']
        cancelled = b.call('capture', {'pid':args.pid,'action':'cancel','jobId':r['jobId']})
        deadline = time.monotonic() + 35
        while cancelled['state'] in ('running','finalizing'):
            if time.monotonic() > deadline: raise TimeoutError('cancel cleanup')
            time.sleep(.1)
            cancelled = a.call('capture', {'pid':args.pid,'action':'status','jobId':r['jobId']})
        evidence['cancelled'] = cancelled
        assert cancelled['state'] == 'cancelled'
        assert a.call('inspect', {'pid':args.pid,'paths':['/project1']})['ok']
        print(json.dumps({'ok':True,'crossSessionBusy':True,'recoverStatus':True,'cancelled':True}))
    finally:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(evidence, indent=2) + '\n')
        a.close()
        b.close()


if __name__ == '__main__':
    main()
