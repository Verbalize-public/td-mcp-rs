"""Live TD timing experiments, loaded as a Text DAT by run_probe.py.

Investigation fixture, not a production scheduler. Requires a disposable TD
project: transport settings affect the entire process. No blocking TD loops.
"""
import json
import time

import td

OWNER = "/project1/timing_probe"


def owner():
    return td.op(OWNER)


def snapshot():
    t = td.root.time
    return {
        "frame": float(t.frame), "seconds": float(t.seconds),
        "absFrame": float(td.absTime.frame), "absSeconds": float(td.absTime.seconds),
        "rate": float(t.rate), "play": bool(t.play),
        "realTime": bool(td.project.realTime), "timePath": t.path,
    }


def setup():
    if owner() is not None:
        raise RuntimeError("Probe already exists; inspect or clean it up first")
    b = td.op('/project1').create(td.baseCOMP, 'timing_probe')
    b.store('original', snapshot())
    b.store('events', [])
    b.store('count', 0)
    b.store('active', False)
    b.appendCustomPage('Controls').appendPulse('Reset')
    fx = b.create(td.baseCOMP, 'fx')
    fx.appendCustomPage('Controls').appendPulse('Reset')
    fx.store('count', 0)
    ramp = fx.create(td.rampTOP, 'ramp')
    fade = fx.create(td.levelTOP, 'fade')
    fade.setInputs([ramp])
    out = fx.create(td.outTOP, 'out1')
    out.setInputs([fade])
    seed = fx.create(td.constantTOP, 'seed')
    fb = fx.create(td.feedbackTOP, 'feedback')
    fb.setInputs([seed])
    inc = fx.create(td.constantTOP, 'increment')
    acc = fx.create(td.compositeTOP, 'accumulated')
    acc.setInputs([fb, inc])
    hist = fx.create(td.outTOP, 'history')
    hist.setInputs([acc])
    for i, n in enumerate(fx.children):
        n.nodeX = (i % 4) * 180
        n.nodeY = -(i // 4) * 130
        n.viewer = False
    callback = b.create(td.executeDAT, 'callbacks')
    callback.par.active = False
    callback.text = "def onFrameStart(frame):\n    parent().op('module').module.on_start(frame)\ndef onFrameEnd(frame):\n    parent().op('module').module.on_end(frame)\n"
    for comp in (b, fx):
        reset = comp.create(td.parameterexecuteDAT, 'reset_callback')
        reset.par.active = False
        reset.text = "def onPulse(par):\n    op('/project1/timing_probe/module').module.reset_signal(par.owner)\n"
    return {"created": b.path, "original": b.fetch('original')}


def on_start(frame):
    b = owner()
    if b is None or not b.fetch('active', False):
        return
    fx = b.op('fx')
    fx.store('count', fx.fetch('count', 0) + 1)
    b.fetch('events').append({'phase': 'start', **snapshot(), 'count': fx.fetch('count')})


def on_end(frame):
    b = owner()
    if b is None or not b.fetch('active', False):
        return
    b.fetch('events').append({'phase': 'end', **snapshot(), 'count': b.op('fx').fetch('count')})


def reset_signal(comp):
    b = owner()
    if comp.path == b.path:
        comp.op('fx').par.Reset.pulse()
    else:
        comp.store('count', 0)
        comp.op('feedback').par.resetpulse.pulse()
    b.store('resets', b.fetch('resets', []) + [comp.path])


def status():
    b = owner()
    return {'time': snapshot(), 'events': b.fetch('events', []),
            'count': b.op('fx').fetch('count', 0), 'resets': b.fetch('resets', []),
            'job': b.fetch('job', None), 'errors': b.errors(recurse=True)}


def configure():
    b = owner()
    td.root.time.play = False
    td.project.realTime = False
    fx = b.op('fx')
    for name in ('ramp', 'seed', 'increment'):
        n = fx.op(name)
        n.par.resolutionw = 64
        n.par.resolutionh = 64
        n.par.format = 'rgba32float'
    for channel in ('colorr', 'colorg', 'colorb'):
        setattr(fx.op('seed').par, channel, 0)
        setattr(fx.op('increment').par, channel, 1 / 120 if channel == 'colorr' else 0)
    fx.op('feedback').par.top = 'accumulated'
    fx.op('accumulated').par.operand = 'add'
    fx.op('increment').par.colorr.expr = "1 / 120 if parent().fetch('count', 0) > 0 else 0"
    fx.op('fade').par.brightness1.expr = "parent().fetch('count', 0) / parent().fetch('duration', 6)"
    for comp in (b, fx):
        cb = comp.op('reset_callback')
        cb.par.op.expr = 'parent()'
        cb.par.pars = 'Reset'
        cb.par.valuechange = False
        cb.par.active = True
    cb = b.op('callbacks')
    cb.par.framestart = True
    cb.par.frameend = True
    cb.par.active = True
    b.par.Reset.pulse()
    movie = b.op('movie') or b.create(td.moviefileoutTOP, 'movie')
    movie.setInputs([fx.op('out1')])
    movie.par.record = False
    return {'time': snapshot(), 'resets': b.fetch('resets', []),
            'movieParameters': [p.name for p in movie.pars()],
            'movieTypes': list(movie.par.type.menuNames),
            'codecs': list(movie.par.videocodec.menuNames),
            'pixelFormats': list(fx.op('accumulated').par.format.menuNames)}


def observe(label):
    import hashlib
    fx = owner().op('fx')
    result = {'label': label, **snapshot(), 'count': fx.fetch('count', 0)}
    for name in ('out1', 'history'):
        node = fx.op(name)
        pixels = node.numpyArray(delayed=False)
        result[name] = {'redMean': float(pixels[:, :, 0].mean()),
                        'hash': hashlib.sha256(pixels.tobytes()).hexdigest(),
                        'cookFrame': float(node.cookFrame),
                        'cookAbsFrame': float(node.cookAbsFrame),
                        'totalCooks': int(node.totalCooks),
                        'timePath': node.time.path}
    return result


def loop_probe(frames=5):
    b = owner()
    b.store('events', [])
    b.store('active', True)
    before = observe('before')
    for _ in range(frames):
        td.root.time.frame += 1
    after = observe('after')
    b.store('active', False)
    return {'before': before, 'after': after, 'events': b.fetch('events')}


def start_steps(frames=6, mechanism='assign', reset=True, directory=None, record=False, codec='rle'):
    import os
    b = owner()
    if b.fetch('job', {}).get('state') == 'running':
        raise RuntimeError('Probe already running')
    if type(frames) is not int or not 1 <= frames <= 120:
        raise ValueError('Probe frames must be between 1 and 120')
    if mechanism not in ('assign', 'play'):
        raise ValueError('Unknown advancement mechanism')
    if record:
        if not directory:
            raise ValueError('Recording requires a directory')
        if codec not in b.op('movie').par.videocodec.menuNames:
            raise ValueError('Codec is not available')
        if os.path.exists(os.path.join(directory, 'recording.mov')):
            raise ValueError('Recording already exists; choose a fresh directory')
    b.store('active', False)
    td.root.time.play = False
    if reset:
        b.par.Reset.pulse()
    b.op('fx').store('duration', frames)
    b.store('events', [])
    b.store('job', {'state': 'running', 'frames': frames, 'mechanism': mechanism,
                    'samples': [], 'deadline': time.monotonic() + 180,
                    'directory': directory, 'record': record, 'codec': codec})
    td.run(prepare_initial, delayFrames=1,
           delayRef=td.op.TDResources, group='tdmcp-timing-probe')
    return {'started': True, 'frames': frames, 'mechanism': mechanism}


def prepare_initial():
    # One explicit initialization frame consumes the reset pulse with the
    # model's counter disabled. Sample zero begins after that full frame.
    if owner() is None or owner().fetch('job', {}).get('state') != 'running':
        return
    td.root.time.frame += 1
    td.run(initial_sample, delayFrames=1, endFrame=True,
           delayRef=td.op.TDResources, group='tdmcp-timing-probe')


def initial_sample():
    if owner() is None or owner().fetch('job', {}).get('state') != 'running':
        return
    try:
        _initial_sample()
    except Exception as exc:
        owner().fetch('job')['error'] = str(exc)
        finish('failed')


def _initial_sample():
    b = owner()
    job = b.fetch('job')
    if job['record']:
        import os
        if not job['directory']:
            finish('failed')
            raise ValueError('Recording requires a directory')
        os.makedirs(job['directory'], exist_ok=True)
        movie = b.op('movie')
        movie.par.type = 'stopframemovie'
        movie.par.videocodec = job['codec']
        movie.par.uniquesuff = False
        job['moviePath'] = os.path.join(job['directory'], 'recording.mov')
        if os.path.exists(job['moviePath']):
            finish('failed')
            raise ValueError('Recording already exists; choose a fresh directory')
        movie.par.file = job['moviePath']
        movie.par.fps = td.root.time.rate
        movie.par.pause = True
        movie.par.record = True
    collect_sample(0)
    b.store('active', True)
    td.run(advance, delayFrames=1, delayRef=td.op.TDResources, group='tdmcp-timing-probe')


def advance():
    b = owner()
    if b is None or b.fetch('job', {}).get('state') != 'running':
        return
    job = b.fetch('job')
    if time.monotonic() > job['deadline']:
        finish('timeout')
        return
    if job['mechanism'] == 'assign':
        td.root.time.frame += 1
    elif job['mechanism'] == 'play':
        td.root.time.play = True
    td.run(after_step, delayFrames=1, endFrame=True, group='tdmcp-timing-probe', delayRef=td.op.TDResources)


def after_step():
    b = owner()
    if b is None or b.fetch('job', {}).get('state') != 'running':
        return
    job = b.fetch('job')
    td.root.time.play = False
    try:
        collect_sample(len(job['samples']))
        if len(job['samples']) > job['frames']:
            # addframe is consumed after this callback. Closing record here
            # dropped the last image in the live test despite force-cooking.
            b.store('active', False)
            td.run(finalize, delayFrames=1, endFrame=True,
                   delayRef=td.op.TDResources, group='tdmcp-timing-probe')
        else:
            td.run(advance, delayFrames=1, delayRef=td.op.TDResources, group='tdmcp-timing-probe')
    except Exception as exc:
        job['error'] = str(exc)
        finish('failed')


def finalize():
    if owner() is not None and owner().fetch('job', {}).get('state') == 'running':
        finish('complete')


def finish(state):
    b = owner()
    job = b.fetch('job')
    job['state'] = state
    b.store('active', False)
    td.root.time.play = False
    movie = b.op('movie')
    if movie is not None and bool(movie.par.record):
        movie.par.record = False
        movie.cook(force=True)
        job['writeCount'] = int(movie.writeCount)
        job['movieErrors'] = movie.errors()


def collect_sample(offset):
    import os
    import base64
    from tdmcp_bridge.capture import handle_capture
    from tdmcp_bridge.inspect import handle_inspect
    b = owner()
    job = b.fetch('job')
    sample = observe(offset)
    directory = job['directory']
    if directory and offset in (0, job['frames'] // 2, job['frames']):
        os.makedirs(directory, exist_ok=True)
        capture = handle_capture({'path': b.op('fx/out1').path, 'mode': 'top', 'maxSize': 64})
        png = os.path.join(directory, f'sample-{offset}.png')
        with open(png, 'wb') as stream:
            stream.write(base64.b64decode(capture.pop('imageBase64')))
        sample['capture'] = capture
        sample['imagePath'] = png
        sample['inspect'] = handle_inspect({'paths': [b.op('fx/fade').path],
                                           'include':['params','errors','warnings']})
        sample['frameAfterInspect'] = float(td.root.time.frame)
    if job['record']:
        movie = b.op('movie')
        movie.par.addframe.pulse()
        movie.cook(force=True)
        sample['writeCount'] = int(movie.writeCount)
        if movie.errors():
            raise RuntimeError(movie.errors())
    job['samples'].append(sample)


def cancel():
    if owner().fetch('job', {}).get('state') == 'running':
        finish('cancelled')
    for task in list(td.runs):
        if task.group == 'tdmcp-timing-probe':
            task.kill()
    return status()


def cleanup():
    b = owner()
    if b.fetch('job', {}).get('state') == 'running':
        cancel()
    original = b.fetch('original')
    b.store('active', False)
    b.op('callbacks').par.active = False
    for run in list(td.runs):
        if run.group == 'tdmcp-timing-probe':
            run.kill()
    td.project.realTime = original['realTime']
    td.root.time.play = original['play']
    b.destroy()
    return {'removed': OWNER, 'time': snapshot(),
            'note': 'Play/realtime restored; timeline/history are not rewound.'}
