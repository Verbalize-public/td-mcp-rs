# Timing investigation examples

These are live-tested investigation fixtures, **not production tools or a
general-purpose scheduler**. Run only against a discovered PID for an owned,
disposable TouchDesigner project. They change the process-wide transport.
The bridge must include the native-stream thread guard and pause-pump fix.

## Implemented-tool acceptance

The shared production runner is now `bridge/tdmcp_bridge/timing.py`. After
setup/configure below, verify it with these **real MCP** acceptance scripts:

```sh
python3 scripts/timing_probe/live_acceptance.py --pid PID --output FRESH_DIRECTORY
python3 scripts/timing_probe/live_failures.py --pid PID --output failures.json
python3 scripts/timing_probe/live_local.py --pid PID --output local.json
python3 scripts/timing_probe/live_ownership.py --pid PID --output ownership.json
```

With the refreshed bootstrap in the owned project, test a daemon restart
without playing the timeline or using UI recovery:

```sh
python3 scripts/timing_probe/live_restart.py begin --pid PID --output restart.json
# Restart the daemon using the repository's documented workflow.
python3 scripts/timing_probe/live_restart.py verify --pid PID --output restart.json
```

The reconnect watchdog uses both `delayRef=op.TDResources` and `wallTime=True`:
the latter bases its delay on elapsed time rather than advancing frames
([Derivative run API](https://docs.derivative.ca/Run_Command_Examples)).

Run sequentially, on a scratch timeline with enough remaining range. These
cover pixel-identical reset replays, sparse feedback sampling, paired inspect,
N-frame MOV download/decode, cancel, source/transport/encoder failure, deadlines,
local-time capture/record with unchanged root frame, and cross-session ownership.
The local-time fixture copies a populated root Time COMP into `local/time`:
creating an empty timeCOMP is not equivalent to its predefined internal network
([Derivative Time COMP](https://docs.derivative.ca/Time_COMP)). Never control a
system/scheduler clock to make a test pass.

The old `start_steps` experiment below is kept as evidence of tested assumptions,
not as a substitute for the production tool. A production record starts its
temporary Movie File Out before sampling, wires it as a sibling of the source,
yields after the last Add Frame, then waits for native container finalization.
Cross-network `setInputs` silently dropped the wire in the live test; checking
the MOV immediately after record-off was also too early.

## Run through MCP

Use `python3 scripts/mcp_probe.py fleet` to discover the scratch PID. Substitute
that PID and fresh evidence/output directories below. The output path in
`--args` is interpreted **inside TD**; under Wine, `/tmp/example` is normally
`Z:/tmp/example`. The host `--output` path is separate.

```sh
python3 scripts/timing_probe/run_probe.py --pid PID --output EVIDENCE setup
python3 scripts/timing_probe/run_probe.py --pid PID --output EVIDENCE configure
python3 scripts/timing_probe/run_probe.py --pid PID --output EVIDENCE loop_probe
python3 scripts/timing_probe/run_probe.py --pid PID --output EVIDENCE start_steps --args '{"frames":6,"directory":"TD_OUTPUT_DIRECTORY","record":true}'
python3 scripts/timing_probe/run_probe.py --pid PID --output EVIDENCE status
```

Poll `status` until terminal; polling does not drive advancement. Inspect the
saved JSON's `ok`, `result.job.state`, `error`, and `movieErrors`. Six advances
produce **seven samples**, offsets 0–6. The production N-frame record API exposes
N samples, not this experiment's advance count. Use a new directory for each
replay: existing `recording.mov` files are rejected before transport mutation.

```sh
ffprobe -v error -count_frames -show_entries stream=codec_name,width,height,r_frame_rate,nb_read_frames,duration -of json HOST_OUTPUT_DIRECTORY/recording.mov
python3 scripts/timing_probe/run_probe.py --pid PID --output EVIDENCE cancel
python3 scripts/timing_probe/run_probe.py --pid PID --output EVIDENCE cleanup
```

Do not run ffprobe before terminal finalization: the movie header may not yet
exist. Cleanup removes only `/project1/timing_probe`, stops its callbacks and
recorder, and restores original play/realtime flags. It cannot restore prior
simulation history or timeline position. Output files are retained.

## Verified patterns (TD 2025.32460, Wine/Linux)

- `td_probe.py::loop_probe`: assigning five successive frames in one Python
  call advanced the displayed frame but ran no frame callbacks or state steps.
- `prepare_initial`: the public parent Reset reaches the child's Reset. One
  explicitly accounted initialization frame consumes the feedback reset with
  the counter disabled; initialized offset zero is black with zero history.
- `advance` / `after_step`: single forward increments, each followed by a full
  TD callback iteration using `delayRef=td.op.TDResources`, gave counters 0–6,
  six start/end callback pairs and monotonically advancing fade/history.
  Sampling in the same end-frame callback as assignment was one step early.
- `collect_sample`: real bridge capture and inspect handlers run together on
  the TD main thread at offsets 0, 3, 6. Inspection did not change the frame.
- Resetting from accumulated history and replaying gave identical SHA-256
  pixel hashes at all seven samples for both outputs, without rewinding time.
- Stop-frame Movie File Out with `rle` produced a decodable 64×64, 60 FPS,
  seven-frame qtrle movie. Decoded red means progressed from 0 to 0.5.
- `finalize`: yielding a callback iteration after the final Add Frame was
  necessary. Immediate closure produced only six frames and omitted the last
  image despite `cook(force=True)`. `writeCount` was not an encoded-frame count;
  verify the finalized file independently.
- A main-thread cancellation scheduled eight TDResources frames after start
  stopped a 121-sample run after two collected samples and finalized a readable
  two-frame partial movie. Invalid counts/mechanisms and an existing output
  file were rejected without changing frame or pulsing reset.

Local evidence from this investigation lives in
`/tmp/tdmcp-timing-ewhgIN/evidence2/run-a` (early closure), `run-b` (corrected),
and the corresponding movie/PNG directories. These temporary files are not
portable test fixtures; the scripts above reproduce the checks.

## Scope and limits

The old prototype's `play` mechanism remains experimental. Production uses
single-frame forward advancement, with separate initialization and warmup.
The acceptance scripts above cover local clocks, interference, source/encoder
failures, cancellation, deadlines, cross-session ownership and reload cleanup.
Unit tests cover range rejection and finalization races. Federation artifact
routing has fake-peer integration coverage, not a two-host native recording run.
Neither a deadline nor a wall-time callback can rescue a blocked TD main thread.
Other codecs, audio, native Windows/macOS and broader media formats remain
outside this backend's verified scope. Never promote the old investigation
fixture directly into tool handlers.

For the separate bridge regression use
`scripts/live_thread_guard_smoke.py PID --output FILE --pause --non-realtime`.
Workers in that example touch only Python streams; all TD work and callback
scheduling stays on the main thread.
