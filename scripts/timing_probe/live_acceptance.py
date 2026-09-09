#!/usr/bin/env python3
"""Verify the shipped timing tools through MCP on an owned scratch fixture.

First run run_probe.py setup/configure. This changes only timing_probe plus
the scratch process transport. ffprobe/ffmpeg on the host verify downloaded
video bytes, so the test also exercises artifact retrieval (not TD paths).
"""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mcp_probe import McpClient


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', required=True, type=int)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = McpClient()
    evidence = {}
    active = None
    def call(tool, params):
        raw = client.request('tools/call', {'name': tool, 'arguments': {'pid': args.pid, **params}})
        if raw.get('isError'):
            raise RuntimeError(raw)
        return raw
    def job(tool, params, name):
        nonlocal active
        raw = call(tool, params)
        active = (tool, raw['structuredContent']['jobId'])
        deadline = time.monotonic() + 150
        while raw['structuredContent']['state'] in ('running', 'finalizing'):
            if time.monotonic() >= deadline:
                raise TimeoutError(name)
            time.sleep(.1)  # polling only, never the TD advancement mechanism
            raw = call(tool, {'action': 'status', 'jobId': active[1]})
        evidence[name] = raw['structuredContent']
        (args.output / f'{name}.json').write_text(json.dumps(raw, indent=2) + '\n')
        assert raw['structuredContent']['state'] == 'complete', raw['structuredContent']
        active = None
        return raw
    try:
        fleet = client.call('fleet', {'pids': [args.pid]})
        assert any(p['pid'] == args.pid and p['bridge'] == 'connected' for p in fleet['processes'])
        # Network reset contract: one initialization frame, then one counter
        # update per frame-start. The component has no knowledge of job internals.
        script = """
b = op('/project1/timing_probe')
if b is None:
    raise RuntimeError('Create the owned timing_probe fixture first')
b.op('fx').store('duration', 6)
b.op('callbacks').text = "def onFrameStart(frame):\\n    fx = parent().op('fx')\\n    if root.time.frame > fx.fetch('origin', float('inf')):\\n        fx.store('count', fx.fetch('count', 0) + 1)\\n"
b.op('fx/reset_callback').text = "def onPulse(par):\\n    fx = par.owner\\n    fx.store('count', 0)\\n    fx.store('origin', root.time.frame + 1)\\n    fx.op('feedback').par.resetpulse.pulse()\\n"
b.op('callbacks').par.frameend = False
result = {'frame':float(root.time.frame), 'end':float(root.time.par.end)}
"""
        call('execute_python', {'script': script})
        reset = {'path': '/project1/timing_probe', 'parameter': 'Reset'}
        request = {'path': '/project1/timing_probe/fx/out1', 'maxSize': 64,
                   'timing': {'reset': reset, 'sampleFrames': [0, 3, 6]},
                   'inspect': {'paths': ['/project1/timing_probe/fx/fade'], 'include': ['params', 'errors']}}
        a = job('capture', request, 'capture-a')
        b = job('capture', request, 'capture-b')
        def hashes(raw):
            return [hashlib.sha256(base64.b64decode(c['data'])).hexdigest()
                    for c in raw['content'] if c['type'] == 'image']
        assert len(hashes(a)) == 3 and hashes(a) == hashes(b)
        assert len(set(hashes(a))) == 3, 'fade did not progress'
        samples = a['structuredContent']['samples']
        assert [s['offset'] for s in samples] == [0, 3, 6]
        assert [s['timing']['frame'] - samples[0]['timing']['frame'] for s in samples] == [0, 3, 6]
        for sample in samples:
            node = sample['inspect']['nodes'][0]
            assert node['timing']['frame'] == sample['timing']['frame']
            brightness = next(p['val'] for p in node['params'] if p['name'] == 'brightness1')
            assert abs(brightness - sample['offset'] / 6) < 1e-6, (sample['offset'], brightness)
        history_request = {'path': '/project1/timing_probe/fx/history', 'maxSize': 64,
                           'timing': {'reset': reset, 'sampleFrames': [0, 3, 6]}}
        ha = job('capture', history_request, 'history-a')
        hb = job('capture', history_request, 'history-b')
        assert hashes(ha) == hashes(hb) and len(set(hashes(ha))) == 3
        movie = job('record', {'path': request['path'], 'frames': 7, 'timing': {'reset': reset}}, 'record')
        metadata = movie['structuredContent']
        assert metadata['artifact']['frames'] == 7
        destination = args.output / 'recording.mov'
        offset = 0
        with destination.open('xb') as stream:
            while True:
                chunk = call('record', {'action': 'read', 'jobId': metadata['jobId'],
                                       'offset': offset, 'length': 4096})['structuredContent']
                data = base64.b64decode(chunk['dataBase64'])
                stream.write(data)
                assert chunk['nextOffset'] == offset + len(data)
                offset = chunk['nextOffset']
                if chunk['eof']:
                    break
        probe = json.loads(subprocess.check_output(['ffprobe', '-v', 'error', '-count_frames',
                '-show_entries', 'stream=codec_name,width,height,r_frame_rate,nb_read_frames', '-of', 'json', str(destination)]))
        evidence['ffprobe'] = probe
        assert int(probe['streams'][0]['nb_read_frames']) == 7
        decoded = subprocess.check_output(['ffmpeg', '-v', 'error', '-i', str(destination),
                                          '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'])
        stride = 64 * 64 * 3
        means = [sum(decoded[i:i+stride:3]) / (64*64*255) for i in range(0, len(decoded), stride)]
        assert len(means) == 7 and abs(means[0]) < .01 and abs(means[-1] - .5) < .01, means
        evidence['decodedRedMeans'] = means
        evidence['replayHashes'] = hashes(a)
        print(json.dumps({'ok': True, 'captureReplay': True, 'feedbackReplay': True,
                          'frames': 7, 'decodedRedMeans': means, 'output': str(args.output)}, indent=2))
    finally:
        if active:
            try:
                call(active[0], {'action': 'cancel', 'jobId': active[1]})
            except Exception as exc:
                evidence['cancelError'] = str(exc)
        (args.output / 'summary.json').write_text(json.dumps(evidence, indent=2) + '\n')
        client.close()


if __name__ == '__main__':
    main()
