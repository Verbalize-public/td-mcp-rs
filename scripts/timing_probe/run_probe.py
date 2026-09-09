#!/usr/bin/env python3
"""Run curated timing probes through the real MCP transport.

Use only a discovered PID belonging to a disposable scratch project.
Example: python3 scripts/timing_probe/run_probe.py --pid PID --output DIR setup
The host polls for completion; polling does not drive TD advancement.
"""
import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mcp_probe import McpClient


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', required=True, type=int)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('action')
    parser.add_argument('--args', default='{}')
    args = parser.parse_args()
    client = McpClient()
    try:
        source = Path(__file__).with_name('td_probe.py').read_text()
        if args.action == 'setup':
            script = f"ns = {{}}\nexec({source!r}, ns)\nresult = ns['setup']()\nb = op('/project1/timing_probe')\nb.create(td.textDAT, 'module').text = {source!r}\n"
        else:
            script = f"dat = op('/project1/timing_probe/module')\nif dat.text != {source!r}:\n    dat.text = {source!r}\nresult = dat.module.{args.action}(**{json.loads(args.args)!r})"
        response = client.call('execute_python', {'pid': args.pid, 'script': script})
        args.output.mkdir(parents=True, exist_ok=True)
        destination = args.output / f'{args.action}.json'
        destination.write_text(json.dumps(response, indent=2) + '\n')
        if args.action in ('status', 'cancel') and response.get('ok'):
            r = response['result']
            job = r.get('job') or {}
            print(json.dumps({'state': job.get('state'), 'error':job.get('error'),
                              'count':r['count'], 'resets':r['resets'], 'errors':r['errors'],
                              'events':len(r['events']), 'samples':[
                                  {'offset':s['label'], 'frame':s['frame'], 'count':s['count'],
                                   'fade':s['out1']['redMean'], 'feedback':s['history']['redMean']}
                                  for s in job.get('samples', [])]}, indent=2))
        else:
            print(json.dumps(response, indent=2))
    finally:
        client.close()


if __name__ == '__main__':
    main()
