"""Main-thread, reset-and-advance jobs shared by capture and recording.

The bridge owns the transport reservation, not the lifetime of an MCP call.
Callbacks always yield to TD; no worker is allowed to enter its API.
"""
from __future__ import annotations

import base64
import builtins
import json
import math
import os
import sys
import tempfile
import time
import uuid
from collections import OrderedDict

from . import state
from .paths import resolve_op

# The baked bootstrap removes every tdmcp_bridge module from sys.modules before
# importing a new generation. Old td.run callables still hold their globals.
# An interpreter-local anchor survives that purge and retires old jobs BEFORE
# a new generation can admit work. This also supports existing baked projects.
_previous_runtime = getattr(builtins, '_tdmcp_timing_runtime', None)
if _previous_runtime is not None and _previous_runtime is not sys.modules[__name__]:
    _previous_runtime.shutdown()
elif '_jobs' in globals():
    shutdown()  # importlib.reload: retire before replacing this module's globals
del _previous_runtime  # do not retain a chain of old modules and sample payloads

MAX_ADVANCES = 3600
MAX_SAMPLES = 16
MAX_RESULT_BYTES = 8 * 1024 * 1024
MAX_VIDEO_BYTES = 512 * 1024 * 1024
CHUNK_BYTES = 256 * 1024
MAX_JOBS = 8
_jobs = OrderedDict()
_active = None


class TimingError(ValueError):
    def __init__(self, message, code='tdmcp.timing.invalid'):
        super().__init__(message)
        self.code = code


def failure(exc):
    return {'ok': False, 'code': getattr(exc, 'code', 'tdmcp.timing.failed'),
            'message': str(exc)}


def snapshot(node=None):
    """Best-effort observation only; never cook or change transport."""
    out = {}
    try:
        import td
        clock = node.time if node is not None else td.root.time
        out['timePath'] = str(clock.path)
        for key in ('frame', 'seconds', 'rate'):
            value = float(getattr(clock, key))
            if math.isfinite(value):
                out[key] = value
        out['play'] = bool(clock.play)
        out['realTime'] = bool(td.project.realTime)
        out['absFrame'] = float(td.absTime.frame)
        out['absSeconds'] = float(td.absTime.seconds)
    except Exception:
        out['available'] = False
    if node is not None:
        for key in ('cookFrame', 'cookAbsFrame', 'totalCooks'):
            try:
                value = float(getattr(node, key))
                if math.isfinite(value):
                    out[key] = value
            except Exception:
                pass
    return out


def integer(value, name, low, high):
    if type(value) is not int or not low <= value <= high:
        raise TimingError(f'{name} must be an integer in {low}..{high}')
    return value


def offsets(options):
    explicit = options.get('sampleFrames')
    repeat = options.get('repeat')
    step = options.get('stepFrames')
    if sum(v is not None for v in (explicit, repeat, step)) > 1:
        raise TimingError('Choose sampleFrames, repeat, or stepFrames, not several')
    if repeat is not None:
        count = integer(repeat.get('count'), 'repeat.count', 1, MAX_SAMPLES)
        interval = integer(repeat.get('interval'), 'repeat.interval', 1, MAX_ADVANCES)
        explicit = [i * interval for i in range(count)]
    if step is not None:
        explicit = [integer(step, 'stepFrames', 1, MAX_ADVANCES)]
    values = [0] if explicit is None else explicit
    if not isinstance(values, list) or not 1 <= len(values) <= MAX_SAMPLES:
        raise TimingError(f'sampleFrames requires 1..{MAX_SAMPLES} offsets')
    for value in values:
        integer(value, 'sampleFrames offset', 0, MAX_ADVANCES)
    if values != sorted(set(values)):
        raise TimingError('sampleFrames must be strictly increasing; replay from reset to go back')
    return values


def admission(method, params):
    """Reject conflicting calls across all MCP sessions for this TD process."""
    if _active is None or method == 'ping':
        return None
    job = _jobs[_active]
    if method == job.kind and params.get('action') in ('status', 'cancel'):
        if params.get('jobId') in (None, job.id):
            return None
    return {'ok': False, 'code': 'tdmcp.timing.busy',
            'message': f'Timing job {job.id} owns this PID; use {job.kind} status/cancel',
            'jobId': job.id, 'tool': job.kind}


def _discard(job):
    # Only bridge-created private artifacts are removed; never caller paths.
    if job.directory:
        if os.path.isfile(job.movie_path):
            os.unlink(job.movie_path)
        os.rmdir(job.directory)


def handle(kind, params):
    state.require_main_thread()
    try:
        action = params.get('action') or 'start'
        if kind == 'capture' and action == 'read':
            raise TimingError('Artifact read is only available on record')
        if action == 'start':
            blocked = admission(kind, params)
            if blocked:
                return blocked
            job = Job(kind, params)  # all preflight before transport mutation
            while len(_jobs) >= MAX_JOBS:
                _, old = next(iter(_jobs.items()))
                _discard(old)
                _jobs.pop(old.id)
            _jobs[job.id] = job
            job.start()
            return job.response()
        job_id = params.get('jobId')
        if job_id is None and action in ('status', 'cancel'):
            job_id = _active or next((j.id for j in reversed(list(_jobs.values())) if j.kind == kind), None)
        job = _jobs.get(job_id)
        if job is None or job.kind != kind:
            raise TimingError('Unknown/expired jobId for this PID and tool', 'tdmcp.timing.not_found')
        if action == 'cancel':
            job.stop('cancelled')
        elif action == 'read':
            if job.artifact is None:
                raise TimingError('Artifact is not finalized', 'tdmcp.timing.failed')
            offset = integer(params.get('offset', 0), 'offset', 0, job.artifact['bytes'])
            size = integer(params.get('length', CHUNK_BYTES), 'length', 1, CHUNK_BYTES)
            with open(job.movie_path, 'rb') as stream:
                stream.seek(offset)
                chunk = stream.read(size)
            return {'ok': True, 'jobId': job.id, 'artifactId': job.id,
                    'offset': offset, 'nextOffset': offset + len(chunk),
                    'eof': offset + len(chunk) == job.artifact['bytes'],
                    'dataBase64': base64.b64encode(chunk).decode('ascii')}
        elif action == 'release':
            if job.id == _active:
                raise TimingError('Cancel and wait for terminal cleanup before release')
            _discard(job)
            del _jobs[job.id]
            return {'ok': True, 'jobId': job.id, 'released': True}
        elif action != 'status':
            raise TimingError('Unknown job action')
        return job.response()
    except Exception as exc:
        return failure(exc)


class Job:
    def __init__(self, kind, params):
        import td
        self.kind, self.params = kind, dict(params)
        self.id = uuid.uuid4().hex
        self.group = 'tdmcp-timing-' + self.id
        self.options = params.get('timing') or {}
        self.source = resolve_op(params.get('path') or '', params.get('contextPath'))
        if self.source is None or not self.source.valid:
            raise TimingError('An existing explicit output path is required')
        if self.source.family != 'TOP':
            raise TimingError('Timed capture/record requires one explicit TOP output')
        if kind == 'capture' and params.get('mode', 'auto') not in ('auto', 'top'):
            raise TimingError('Timed capture supports top/auto only')
        size = params.get('maxSize', 512)
        if kind == 'capture' and size is not None:
            integer(size, 'maxSize', 1, 1536)
        if kind == 'capture' and size is None and max(self.source.width, self.source.height) > 1536:
            raise TimingError('Native capture exceeds 1536px; provide maxSize')
        self.clock = self.source.time
        self.path, self.time_path = self.source.path, self.clock.path
        selected = self.options.get('timePath')
        if selected:
            comp = resolve_op(selected, params.get('contextPath'))
            if comp is None:
                raise TimingError('timePath does not exist')
            clock = comp if hasattr(comp, 'frame') and hasattr(comp, 'play') else comp.time
            if clock.path != self.clock.path:
                raise TimingError('timePath must be the output\'s effective time source')
        self.delay_ref = td.op.TDResources
        if self.delay_ref is None:
            raise TimingError('No pause-independent TDResources time reference')
        if self.clock.path.startswith('/sys/') or self.clock.path == self.delay_ref.time.path:
            raise TimingError('Cannot take ownership of the scheduler time reference')
        self.rate = float(self.clock.rate)
        if not math.isfinite(self.rate) or self.rate <= 0:
            raise TimingError('Timeline rate must be finite and positive')
        self.after = self.options.get('after', 'pause')
        if self.after not in ('pause', 'play', 'restore'):
            raise TimingError('after must be pause, play, or restore')
        self.reset_par = None
        reset = self.options.get('reset')
        if reset:
            comp = resolve_op(reset.get('path', ''), params.get('contextPath'))
            self.reset_par = getattr(comp.par, reset.get('parameter', ''), None) if comp else None
            if self.reset_par is None or not self.reset_par.isPulse:
                raise TimingError('reset must select an existing Pulse parameter')
        self.init_frames = integer(self.options.get('initializeFrames', 1 if reset else 0),
                                   'initializeFrames', 0, 600)
        self.warmup = integer(self.options.get('warmupFrames', 0), 'warmupFrames', 0, 600)
        self.requested = offsets(self.options) if kind == 'capture' else list(range(
            integer(params.get('frames'), 'frames', 1, MAX_ADVANCES)))
        if kind == 'record' and any(self.options.get(k) is not None for k in ('sampleFrames', 'repeat', 'stepFrames')):
            raise TimingError('record uses frames consecutive samples, not a capture schedule')
        self.timeout = integer(self.options.get('timeoutSeconds', 120), 'timeoutSeconds', 1, 600)
        self.expected = float(self.clock.frame)
        if not self.expected.is_integer():
            raise TimingError('The effective timeline must be at an integer frame')
        self.range_end = float(self.clock.par.rangeend.eval()) if hasattr(self.clock.par, 'rangeend') else float(self.clock.par.end.eval())
        self.end = min(float(self.clock.par.end.eval()), self.range_end)
        if self.expected + self.init_frames + self.warmup + self.requested[-1] > self.end:
            raise TimingError('Schedule crosses the timeline end; extend the range explicitly before starting')
        paired = params.get('inspect')
        if paired:
            if not isinstance(paired.get('paths'), list) or not 1 <= len(paired['paths']) <= 16:
                raise TimingError('Paired inspect requires 1..16 paths')
            if 'content' in paired.get('include', []):
                raise TimingError('Paired inspection excludes content (may force shader recompilation)')
            for path in paired['paths']:
                node = resolve_op(path, params.get('contextPath'))
                if node is None or node.time.path != self.clock.path:
                    raise TimingError('Paired inspection paths must exist and share the sampled time source')
        self.original = {'play': bool(self.clock.play), 'rootPlay': bool(td.root.time.play),
                         'realTime': bool(td.project.realTime)}
        self.root_frame = float(td.root.time.frame)
        self.state = 'running'
        self.phase = 'initializing'
        self.progress = 0
        self.initial_remaining = self.init_frames + self.warmup
        self.samples = []
        self.result_bytes = 0
        self.error = None
        self.movie = None
        self.directory = None
        self.movie_path = None
        self.artifact = None
        self.recorded = 0
        self.final_state = None
        self.deadline = 0
        self.flush_deadline = 0

    def schedule(self, phase):
        import td
        td.run(lambda: self.tick(phase), delayFrames=1, endFrame=True,
               delayRef=self.delay_ref, group=self.group)

    def start(self):
        global _active
        import td
        _active = self.id
        self.deadline = time.monotonic() + self.timeout
        try:
            td.root.time.play = False
            self.clock.play = False
            td.project.realTime = False
            if self.kind == 'record':
                # TD must initialize the new Movie File Out and its input in a
                # separate iteration before the first Add Frame is requested.
                self.open_movie()
            if self.reset_par is not None:
                self.reset_par.pulse()
            self.schedule('prepare')  # let parameter callbacks propagate reset
        except Exception as exc:
            self.error = failure(exc)
            self.close('failed')

    def check(self):
        import td
        import tdmcp_bridge
        if not tdmcp_bridge.is_connected():
            raise TimingError('Bridge disconnected; job stopped', 'tdmcp.timing.interrupted')
        if time.monotonic() > self.deadline:
            raise TimingError('Job deadline exceeded', 'tdmcp.timing.interrupted')
        if not self.source.valid or not self.clock.valid or self.source.time.path != self.clock.path:
            raise TimingError('Source/time disappeared or changed', 'tdmcp.timing.interrupted')
        if (self.clock.play or td.root.time.play or td.project.realTime
                or float(self.clock.frame) != self.expected or float(self.clock.rate) != self.rate
                or min(float(self.clock.par.end.eval()), float(self.clock.par.rangeend.eval())
                       if hasattr(self.clock.par, 'rangeend') else self.end) != self.end
                or (self.clock.path != td.root.time.path and float(td.root.time.frame) != self.root_frame)):
            raise TimingError('External transport/time-source change detected', 'tdmcp.timing.interrupted')

    def tick(self, phase):
        state.require_main_thread()
        if _jobs.get(self.id) is not self or _active != self.id:
            return  # stale callback after release/reload/cancellation
        try:
            if phase in ('close', 'verify'):
                self.close(self.final_state)
                return
            if self.state != 'running':
                return
            self.check()
            if phase == 'prepare':
                if self.initial_remaining:
                    self.advance('initialized')
                else:
                    self.sample()
            elif phase == 'initialized':
                self.initial_remaining -= 1
                self.source.cook(force=True)  # consume reset/warmup even without samples
                self.schedule('prepare')
            elif phase == 'advance':
                self.advance('sample')
            elif phase == 'sample':
                self.progress += 1
                self.sample()
        except Exception as exc:
            self.error = failure(exc)
            if self.state == 'running':
                self.stop('failed')
            else:
                self.close('failed', immediate=True)

    def advance(self, next_phase):
        if self.expected + 1 > self.end:
            raise TimingError('Timeline range end reached', 'tdmcp.timing.interrupted')
        self.expected += 1
        self.clock.frame = self.expected
        self.schedule(next_phase)  # full callback iteration before observing

    def sample(self):
        from .capture import handle_capture
        from .inspect import handle_inspect
        self.phase = 'sampling'
        # Demand every intervening output frame, even between sparse captures.
        # A cook in each distinct TD iteration is necessary for feedback history.
        self.source.cook(force=True)
        if self.progress in self.requested:
            sample = {'offset': self.progress, 'timing': snapshot(self.source)}
            if self.kind == 'capture':
                sample['capture'] = handle_capture({k: v for k, v in self.params.items()
                                                     if k in ('path', 'contextPath', 'mode', 'maxSize')})
                if not sample['capture'].get('ok'):
                    raise TimingError(sample['capture'].get('message') or 'Capture failed', 'tdmcp.timing.failed')
                if self.params.get('inspect'):
                    sample['inspect'] = handle_inspect({**self.params['inspect'],
                                                       'contextPath': self.params.get('contextPath')})
                    if any(not n.get('ok') for n in sample['inspect'].get('nodes', [])):
                        raise TimingError('A paired inspection target disappeared', 'tdmcp.timing.interrupted')
                if float(self.clock.frame) != self.expected:
                    raise TimingError('Transport changed during sampling', 'tdmcp.timing.interrupted')
                size = len(json.dumps(sample).encode('utf-8'))
                if self.result_bytes + size > MAX_RESULT_BYTES:
                    raise TimingError('Capture result exceeds 8 MiB; reduce size or sample count')
                self.result_bytes += size
                self.samples.append(sample)
            else:
                if self.movie is None:
                    self.open_movie()
                if self.source.width != self.width or self.source.height != self.height:
                    raise TimingError('Output dimensions changed during recording', 'tdmcp.timing.interrupted')
                self.movie.par.addframe.pulse()
                self.movie.cook(force=True)
                if self.movie.errors():
                    raise TimingError(str(self.movie.errors()), 'tdmcp.timing.failed')
                self.recorded += 1
                if self.progress in (0, self.requested[-1]):
                    self.samples.append(sample)
                if os.path.getsize(self.movie_path) > MAX_VIDEO_BYTES:
                    raise TimingError('Video exceeds 512 MiB artifact limit', 'tdmcp.timing.failed')
        if self.progress == self.requested[-1]:
            self.stop('complete')
        else:
            self.schedule('advance')

    def open_movie(self):
        import td
        # TD silently drops a wire across unrelated COMP interiors. A sibling
        # also inherits the same effective clock as the source (local time too).
        host = self.source.parent()
        self.directory = tempfile.mkdtemp(prefix='tdmcp-record-')
        self.movie_path = os.path.join(self.directory, 'recording.mov')
        self.width, self.height = int(self.source.width), int(self.source.height)
        if not 1 <= self.width <= 4096 or not 1 <= self.height <= 4096:
            raise TimingError('Recording dimensions must be in 1..4096')
        self.movie = host.create(td.moviefileoutTOP, 'record_' + self.id)
        self.movie.viewer = False
        self.movie.comment = 'Temporary tdmcp frame-exact recorder; owned by job ' + self.id
        self.movie.setInputs([self.source])
        if 'rle' not in self.movie.par.videocodec.menuNames:
            raise TimingError('This installation lacks the supported rle codec')
        self.movie.par.type = 'stopframemovie'
        self.movie.par.videocodec = 'rle'
        self.movie.par.uniquesuff = False
        self.movie.par.file = self.movie_path
        self.movie.par.fps = self.rate
        self.movie.par.pause = True
        self.movie.par.record = True

    def stop(self, terminal):
        if self.state == 'cleanup_failed':
            self.close('failed')
            return
        if self.state != 'running':
            return
        self.state, self.phase, self.final_state = 'finalizing', 'finalizing', terminal
        try:
            self.schedule('close')  # last Add Frame must finish before record-off
        except Exception as exc:
            self.error = failure(exc)
            self.close('failed')

    def close(self, terminal, immediate=False):
        global _active
        import td
        # record-off starts native encoder finalization. Its header can arrive
        # asynchronously after this callback; retain ownership until verified.
        if self.movie is not None and self.movie.valid and self.flush_deadline == 0:
            try:
                self.movie.par.record = False
                self.movie.cook(force=True)
                self.flush_deadline = time.monotonic() + 30
                if not immediate:
                    self.schedule('verify')
                    return
            except Exception as exc:
                self.error = failure(exc)
                terminal = 'failed'
        if self.movie is not None and self.movie.valid and self.recorded and not immediate:
            try:
                from .video import read_moov
                read_moov(self.movie_path)
            except (ValueError, OSError):
                if time.monotonic() < self.flush_deadline:
                    self.schedule('verify')
                    return
        cleanup_ok = True
        try:
            for task in list(td.runs):
                if task.group == self.group:
                    task.kill()
            if self.movie is not None and self.movie.valid:
                if self.movie.errors():
                    raise TimingError(str(self.movie.errors()), 'tdmcp.timing.failed')
                if self.recorded and not immediate:
                    from .video import verify_movie
                    self.artifact = verify_movie(self.movie_path, self.recorded,
                                                 self.width, self.height, self.rate)
                    self.artifact.update({'artifactId': self.id, 'mimeType': 'video/quicktime'})
        except Exception as exc:
            self.error = failure(exc)
            terminal = 'failed'
        finally:
            try:
                if self.movie is not None and self.movie.valid:
                    self.movie.destroy()
                    if self.movie.valid:
                        raise TimingError('Recorder survived cleanup; retry cancel', 'tdmcp.timing.failed')
            except Exception as exc:
                self.error = failure(exc)
                cleanup_ok = False
            try:
                td.project.realTime = self.original['realTime']
                # Failure never resumes playback. Restoring flags cannot restore history.
                play = terminal == 'complete' and (self.after == 'play' or
                        (self.after == 'restore' and self.original['play']))
                if self.clock.valid:
                    self.clock.play = play
                if self.clock.path != td.root.time.path:
                    td.root.time.play = terminal == 'complete' and self.after == 'restore' and self.original['rootPlay']
            except Exception as exc:
                self.error = failure(exc)
                terminal = 'failed'
            self.state, self.phase = (terminal, 'done') if cleanup_ok else ('cleanup_failed', 'cleanup')
            if cleanup_ok and _active == self.id:
                _active = None

    def response(self):
        return {'ok': True, 'jobId': self.id, 'kind': self.kind, 'state': self.state,
                'phase': self.phase, 'path': self.path, 'timePath': self.time_path,
                'requestedSamples': len(self.requested), 'advancedFrames': self.progress,
                'initializeFrames': self.init_frames, 'warmupFrames': self.warmup,
                'samples': self.samples, 'recordedSamples': self.recorded,
                'error': self.error, 'artifact': self.artifact}


def shutdown():
    """Called before module reload, on main; no old callback may mutate again."""
    state.require_main_thread()
    if _active is not None:
        job = _jobs[_active]
        job.error = failure(TimingError('Bridge reloaded/disconnected', 'tdmcp.timing.interrupted'))
        job.close('cancelled', immediate=True)
        if _active is not None:
            raise TimingError('Recorder cleanup failed; cannot safely reload bridge', 'tdmcp.timing.failed')
    for job in _jobs.values():
        _discard(job)
        job.directory = None
        job.artifact = None


builtins._tdmcp_timing_runtime = sys.modules[__name__]
