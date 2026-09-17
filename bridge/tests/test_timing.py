"""Deferred timing contracts; no real TD needed. Live acceptance complements these."""
import sys
from pathlib import Path
from types import SimpleNamespace as NS

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import tdmcp_bridge as bridge
from tdmcp_bridge import capture, timing


@pytest.fixture
def runtime(monkeypatch):
    pending, cooked, resets = [], [], []
    clock = NS(path='/local/time', frame=10., seconds=0., rate=60., play=True,
               valid=True, par=NS(end=NS(eval=lambda: 600.)))
    clock.time = clock
    source = NS(path='/project1/out1', valid=True, family='TOP', time=clock,
                width=64, height=64, cookFrame=0, cookAbsFrame=0, totalCooks=0)
    source.cook = lambda **kw: cooked.append(clock.frame)
    reset = NS(isPulse=True, pulse=lambda: resets.append(clock.frame))
    comp = NS(par=NS(Reset=reset), time=clock)
    nodes = {source.path: source, '/project1': comp, clock.path: clock}
    class Ops:
        TDResources = NS(time=NS(path='/sys/TDResources/local/time'))
        def __call__(self, path):
            return nodes.get(path)
    def run(fn, **kwargs):
        task = NS(fn=fn, group=kwargs['group'], alive=True)
        task.kill = lambda: setattr(task, 'alive', False)
        pending.append(task)
        return task
    td = NS(root=NS(time=clock), project=NS(realTime=True), op=Ops(), run=run,
            runs=pending, absTime=NS(frame=100., seconds=2.))
    monkeypatch.setitem(sys.modules, 'td', td)
    monkeypatch.setattr(bridge, 'tdmcp_resolve', lambda path, ctx=None: nodes.get(path))
    monkeypatch.setattr(bridge, 'is_connected', lambda: True)
    monkeypatch.setattr(timing, '_active', None)
    monkeypatch.setattr(timing, '_jobs', timing.OrderedDict())
    monkeypatch.setattr(capture, 'handle_capture', lambda p: {'ok': True, 'imageBase64': 'eA==', 'mimeType': 'image/png'})
    def tick():
        while pending:
            task = pending.pop(0)
            if task.alive:
                task.fn()
                return
    def drain():
        for _ in range(200):
            if not pending:
                return
            tick()
        pytest.fail('runner did not terminate')
    return NS(td=td, clock=clock, source=source, resets=resets, cooked=cooked,
              pending=pending, tick=tick, drain=drain)


def start(**options):
    return timing.handle('capture', {'path': '/project1/out1', 'timing': options})


@pytest.mark.parametrize('options', [
    {'sampleFrames': [1, 0]}, {'sampleFrames': [0, 0]}, {'sampleFrames': [-1]},
    {'sampleFrames': [True]}, {'sampleFrames': []}, {'stepFrames': 0},
    {'sampleFrames': [0], 'repeat': {'count': 2, 'interval': 1}},
    {'repeat': {'count': 17, 'interval': 1}}, {'sampleFrames': [3601]},
    {'timeoutSeconds': 0}, {'after': 'seek'}, {'initializeFrames': 601},
    {'reset': {'path': '/project1', 'parameter': 'Missing'}},
])
def test_invalid_preflight_does_not_change_transport(runtime, options):
    result = start(**options)
    assert not result['ok']
    assert runtime.clock.play and runtime.td.project.realTime
    assert runtime.resets == [] and runtime.pending == []


def test_reset_warmup_and_sparse_samples_cook_every_frame(runtime):
    response = start(reset={'path': '/project1', 'parameter': 'Reset'},
                     initializeFrames=1, warmupFrames=2, sampleFrames=[0, 2, 5])
    assert response['state'] == 'running'
    assert runtime.resets == [10]
    assert runtime.cooked == []  # start yields; RPC does not step a loop
    runtime.drain()
    job = timing._jobs[response['jobId']]
    assert job.state == 'complete'
    assert [s['offset'] for s in job.samples] == [0, 2, 5]
    assert [s['timing']['frame'] for s in job.samples] == [13, 15, 18]
    assert set(runtime.cooked) == set(range(11, 19))
    assert timing._active is None
    assert runtime.clock.play is False and runtime.td.project.realTime is True


def test_range_end_rejected_without_reset(runtime):
    runtime.clock.frame = 599
    assert not start(sampleFrames=[0, 2], reset={'path': '/project1', 'parameter': 'Reset'})['ok']
    assert runtime.resets == [] and runtime.clock.play


def test_working_range_end_and_system_clock_are_protected(runtime):
    runtime.clock.par.rangeend = NS(eval=lambda: 11.)
    assert not start(sampleFrames=[0, 2])['ok']
    runtime.clock.path = '/sys/local/time'
    assert not start()['ok']
    assert runtime.clock.play and not runtime.pending


def test_repeat_step_after_play(runtime):
    r = start(repeat={'count': 3, 'interval': 2}, after='play')
    runtime.drain()
    assert [s['offset'] for s in timing._jobs[r['jobId']].samples] == [0, 2, 4]
    assert runtime.clock.play


def test_ownership_spans_callbacks_and_cancel_cleanup(runtime):
    r = start(sampleFrames=[0, 20])
    assert timing.admission('execute_python', {})['code'] == 'tdmcp.timing.busy'
    assert timing.admission('inspect', {})['jobId'] == r['jobId']
    assert timing.admission('capture', {'action': 'status'}) is None
    assert timing.admission('record', {'action': 'cancel', 'jobId': r['jobId']})
    runtime.tick()
    stopped = timing.handle('capture', {'action': 'cancel', 'jobId': r['jobId']})
    assert stopped['state'] == 'finalizing'
    assert timing.admission('execute_python', {})  # not released before cleanup
    runtime.drain()
    frame = runtime.clock.frame
    assert timing._jobs[r['jobId']].state == 'cancelled'
    assert timing._active is None
    assert timing.handle('capture', {'action': 'cancel', 'jobId': r['jobId']})['state'] == 'cancelled'
    assert runtime.clock.frame == frame


@pytest.mark.parametrize('cause', ['seek', 'play', 'rate', 'source', 'disconnect', 'timeout'])
def test_external_changes_stop_job(runtime, monkeypatch, cause):
    r = start(sampleFrames=[0, 5])
    if cause == 'seek':
        runtime.clock.frame = 123
    elif cause == 'play':
        runtime.clock.play = True
    elif cause == 'rate':
        runtime.clock.rate = 30
    elif cause == 'source':
        runtime.source.valid = False
    elif cause == 'disconnect':
        monkeypatch.setattr(bridge, 'is_connected', lambda: False)
    else:
        timing._jobs[r['jobId']].deadline = 0
    runtime.drain()
    job = timing._jobs[r['jobId']]
    assert job.state == 'failed'
    assert job.error['code'] == 'tdmcp.timing.interrupted'
    assert timing._active is None and not runtime.clock.play


def test_reload_kills_stale_callbacks(runtime):
    r = start(sampleFrames=[0, 4])
    callback = runtime.pending[0].fn
    timing.shutdown()
    assert timing._jobs[r['jobId']].state == 'cancelled'
    frame = runtime.clock.frame
    callback()
    assert runtime.clock.frame == frame


def test_payload_limit_keeps_partial_samples(runtime, monkeypatch):
    monkeypatch.setattr(timing, 'MAX_RESULT_BYTES', 1)
    r = start()
    runtime.drain()
    job = timing._jobs[r['jobId']]
    assert job.state == 'failed' and not job.samples


def test_final_sample_is_not_closed_in_same_callback(runtime):
    r = start()
    runtime.tick()
    assert timing._jobs[r['jobId']].state == 'finalizing'
    assert timing._active == r['jobId']
    runtime.tick()
    assert timing._jobs[r['jobId']].state == 'complete'


def test_snapshot_reports_effective_time_without_cooking(runtime):
    observed = timing.snapshot(runtime.source)
    assert observed['timePath'] == '/local/time' and observed['frame'] == 10
    assert runtime.cooked == [] and runtime.clock.play


def test_effective_local_clock_advanced_without_seeking_root(runtime):
    local = NS(**vars(runtime.clock))
    local.path = '/project1/fx/local/time'
    local.time = local
    runtime.source.time = local
    r = start(sampleFrames=[0, 2], after='restore')
    runtime.drain()
    job = timing._jobs[r['jobId']]
    assert job.state == 'complete'
    assert local.frame == 12 and runtime.clock.frame == 10
    assert local.play and runtime.clock.play


def test_cleanup_failure_keeps_ownership_until_retry(runtime):
    r = start()
    job = timing._jobs[r['jobId']]
    movie = NS(valid=True, par=NS(record=True), errors=lambda: '', cook=lambda **kw: None)
    movie.destroy = lambda: None
    job.movie = movie
    runtime.drain()
    assert job.state == 'cleanup_failed' and timing._active == job.id
    movie.destroy = lambda: setattr(movie, 'valid', False)
    timing.handle('capture', {'action': 'cancel', 'jobId': job.id})
    assert job.state == 'failed' and timing._active is None


def test_artifact_reads_are_bounded_and_release_is_scoped(runtime, tmp_path):
    import base64
    r = start()
    runtime.drain()
    job = timing._jobs[r['jobId']]
    job.kind = 'record'
    private = tmp_path / 'private'
    private.mkdir()
    movie = private / 'recording.mov'
    movie.write_bytes(b'abcdef')
    unrelated = tmp_path / 'keep.mov'
    unrelated.write_bytes(b'keep')
    job.directory, job.movie_path = str(private), str(movie)
    job.artifact = {'bytes': 6}
    request = {'jobId': job.id, 'action': 'read', 'offset': 2, 'length': 3}
    chunk = timing.handle('record', request)
    assert base64.b64decode(chunk['dataBase64']) == b'cde'
    assert chunk['nextOffset'] == 5 and not chunk['eof']
    assert not timing.handle('record', {**request, 'offset': -1})['ok']
    assert not timing.handle('record', {**request, 'length': timing.CHUNK_BYTES + 1})['ok']
    assert timing.handle('record', {'action': 'release', 'jobId': job.id})['released']
    assert not private.exists() and unrelated.read_bytes() == b'keep'
    assert timing.handle('record', request)['code'] == 'tdmcp.timing.not_found'


def test_recorder_yields_before_first_frame_and_waits_for_container(runtime, monkeypatch, tmp_path):
    from tdmcp_bridge import video
    requested, checks = [], []
    movie_path = tmp_path / 'movie.mov'
    movie_path.write_bytes(b'not finalized yet')
    def open_movie(job):
        movie = NS(valid=True, errors=lambda: '', cook=lambda **kw: None)
        movie.par = NS(record=True, addframe=NS(pulse=lambda: requested.append(runtime.clock.frame)))
        movie.destroy = lambda: setattr(movie, 'valid', False)
        job.movie, job.movie_path, job.width, job.height = movie, str(movie_path), 64, 64
    def moov(path):
        checks.append(path)
        if len(checks) == 1:
            raise ValueError('no moov yet')
        return b'moov', 20
    monkeypatch.setattr(timing.Job, 'open_movie', open_movie)
    monkeypatch.setattr(video, 'read_moov', moov)
    monkeypatch.setattr(video, 'verify_movie', lambda *args: {'frames': args[1], 'bytes': 20})
    r = timing.handle('record', {'path': runtime.source.path, 'frames': 3})
    assert requested == [] and timing._jobs[r['jobId']].movie is not None
    runtime.drain()
    job = timing._jobs[r['jobId']]
    assert job.state == 'complete'
    assert requested == [10, 11, 12] and len(checks) == 2
    assert job.artifact['frames'] == 3 and not job.movie.valid


def test_lost_start_reply_can_recover_latest_terminal_job(runtime):
    r = start()
    runtime.drain()
    latest = timing.handle('capture', {'action': 'status'})
    assert latest['jobId'] == r['jobId'] and latest['state'] == 'complete'


def test_status_sample_cursor_is_repeatable_and_indexes_stored_samples(runtime):
    r = start(sampleFrames=[0, 2, 5])
    runtime.drain()
    job = timing._jobs[r['jobId']]
    legacy = timing.handle('capture', {'action': 'status', 'jobId': job.id})
    assert set(legacy) == {
        'ok', 'jobId', 'kind', 'state', 'phase', 'path', 'timePath',
        'requestedSamples', 'advancedFrames', 'initializeFrames', 'warmupFrames',
        'samples', 'recordedSamples', 'error', 'artifact',
    }
    before = (runtime.clock.frame, list(runtime.cooked), job.result_bytes)
    request = {'action': 'status', 'jobId': job.id, 'sampleOffset': 1}
    result = timing.handle('capture', request)
    assert result == timing.handle('capture', request)
    assert result['samples'] == legacy['samples'][1:]
    assert [s['offset'] for s in result['samples']] == [2, 5]
    assert result['sampleOffset'] == 1
    assert result['totalSamples'] == result['nextSampleOffset'] == 3
    assert result['samplesComplete'] is True
    exhausted = timing.handle('capture', {**request, 'sampleOffset': 3})
    assert exhausted['samples'] == [] and exhausted['samplesComplete'] is True
    assert exhausted['nextSampleOffset'] == 3
    explicit = timing.handle('capture', {**request, 'sampleOffset': 0, 'includeSamples': True})
    assert explicit['samples'] == legacy['samples']
    assert before == (runtime.clock.frame, runtime.cooked, job.result_bytes)
    assert timing.handle('capture', {'action': 'status'}) == legacy


def test_metadata_only_status_discovers_job_without_consuming_samples(runtime):
    r = start(sampleFrames=[0, 2])
    request = {'action': 'status', 'includeSamples': False}
    empty = timing.handle('capture', request)
    assert empty['jobId'] == r['jobId'] and empty['samples'] == []
    assert empty['totalSamples'] == empty['nextSampleOffset'] == 0
    assert empty['samplesComplete'] is False
    runtime.tick()
    partial = timing.handle('capture', request)
    assert partial['state'] == 'running' and partial['samples'] == []
    assert partial['totalSamples'] == 1 and partial['nextSampleOffset'] == 0
    assert partial['samplesComplete'] is False
    fetched = timing.handle('capture', {**request, 'includeSamples': True})
    assert len(fetched['samples']) == fetched['nextSampleOffset'] == 1
    assert fetched['samplesComplete'] is False
    caught_up = timing.handle('capture', {**request, 'sampleOffset': 1})
    assert caught_up['samples'] == [] and caught_up['samplesComplete'] is False
    assert timing.admission('inspect', {})['jobId'] == r['jobId']
    runtime.drain()
    terminal = timing.handle('capture', request)
    assert terminal['state'] == 'complete' and terminal['samples'] == []
    assert terminal['totalSamples'] == 2 and terminal['nextSampleOffset'] == 0
    assert terminal['samplesComplete'] is False
    remaining = timing.handle('capture', {'action': 'status', 'sampleOffset': 1})
    assert [s['offset'] for s in remaining['samples']] == [2]
    assert remaining['samples'][0]['capture']['imageBase64'] == 'eA=='
    assert remaining['nextSampleOffset'] == 2 and remaining['samplesComplete'] is True
    assert len(timing._jobs[r['jobId']].samples) == 2


@pytest.mark.parametrize('options', [
    {'sampleOffset': -1}, {'sampleOffset': True}, {'sampleOffset': 0.5},
    {'sampleOffset': '0'}, {'sampleOffset': None}, {'sampleOffset': 2},
    {'sampleOffset': 2**32}, {'includeSamples': 0}, {'includeSamples': 'false'},
    {'includeSamples': None}, {'includeSamples': False, 'sampleOffset': 2},
])
def test_invalid_status_options_do_not_fail_job(runtime, options):
    r = start(sampleFrames=[0, 5])
    runtime.tick()
    job = timing._jobs[r['jobId']]
    before = (runtime.clock.frame, list(runtime.cooked), list(job.samples), job.result_bytes)
    result = timing.handle('capture', {'action': 'status', 'jobId': job.id, **options})
    assert result['ok'] is False and result['code'] == 'tdmcp.timing.invalid'
    assert job.state == 'running' and job.error is None and timing._active == job.id
    assert before == (runtime.clock.frame, runtime.cooked, job.samples, job.result_bytes)
    runtime.drain()
    assert job.state == 'complete' and len(job.samples) == 2


@pytest.mark.parametrize('terminal', ['complete', 'cancelled', 'failed', 'cleanup_failed'])
def test_filtered_status_preserves_state_errors_and_reservation(runtime, terminal):
    r = start()
    runtime.tick()
    job = timing._jobs[r['jobId']]
    finalizing = timing.handle('capture', {'action': 'status', 'sampleOffset': 0})
    assert finalizing['state'] == 'finalizing' and finalizing['samplesComplete'] is False
    runtime.drain()
    job.state = terminal
    job.phase = 'cleanup' if terminal == 'cleanup_failed' else 'done'
    job.error = {'ok': False, 'code': 'tdmcp.timing.failed', 'message': 'kept'}
    if terminal == 'cleanup_failed':
        timing._active = job.id
    result = timing.handle('capture', {'action': 'status', 'includeSamples': False})
    assert result['ok'] is True and result['state'] == terminal and result['phase'] == job.phase
    assert result['error'] == job.error and result['samples'] == []
    assert result['totalSamples'] == 1 and result['samplesComplete'] is False
    fetched = timing.handle('capture', {'action': 'status', 'sampleOffset': 0})
    assert fetched['samples'] == job.samples
    assert fetched['samplesComplete'] is (terminal != 'cleanup_failed')
    assert (timing._active == job.id) is (terminal == 'cleanup_failed')


def test_status_flags_are_request_local_and_capture_only(runtime):
    r = timing.handle('capture', {'path': runtime.source.path, 'timing': {},
                                  'includeSamples': False, 'sampleOffset': 99})
    assert r['state'] == 'running' and 'totalSamples' not in r
    runtime.drain()
    job = timing._jobs[r['jobId']]
    result = timing.handle('capture', {'action': 'status'})
    assert len(result['samples']) == 1 and 'totalSamples' not in result
    job.kind = 'record'
    record = timing.handle('record', {'action': 'status', 'includeSamples': False, 'sampleOffset': 99})
    assert record == {**result, 'kind': 'record'}
    job.kind = 'capture'
    cancel = timing.handle('capture', {'action': 'cancel', 'includeSamples': False, 'sampleOffset': 99})
    assert cancel == result
    assert timing.handle('capture', {'action': 'release', 'jobId': job.id,
                                     'includeSamples': False, 'sampleOffset': 99})['released']
    assert timing.handle('capture', {'action': 'status', 'jobId': job.id,
                                     'includeSamples': False})['code'] == 'tdmcp.timing.not_found'


def test_bootstrap_style_fresh_import_retires_old_callback_generation(runtime, monkeypatch):
    import builtins
    import importlib.util
    r = start(sampleFrames=[0, 5])
    old = timing._jobs[r['jobId']]
    callback = runtime.pending[0].fn
    monkeypatch.setattr(builtins, '_tdmcp_timing_runtime', timing, raising=False)
    spec = importlib.util.spec_from_file_location(timing.__name__, timing.__file__)
    fresh = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, timing.__name__, fresh)
    spec.loader.exec_module(fresh)
    assert old.state == 'cancelled'
    assert not hasattr(fresh, '_previous_runtime')
    frame = runtime.clock.frame
    callback()
    assert runtime.clock.frame == frame and fresh._active is None
