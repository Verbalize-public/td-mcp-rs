# Timed capture and recording

Use for repeatable transitions, feedback/simulation checks, and N-frame video.
Plain `inspect`/`capture` remain immediate observations; inspect now includes
effective-clock and cook metadata. Exact schedules require a `timing` job.

## Reset, initialize, advance

Use the public reset contract ({{ skill("reset-state") }}), never backward
seeking to reconstruct history. Confirm controlled inputs/seeds and which
initialization frames the component needs. A reset pulse defaults to one
`initializeFrames`; override explicitly for the network's documented contract.
Optional `warmupFrames` happen before sample zero, separately accounted.

```json
{
  "pid": 123,
  "path": "/project1/fade/out1",
  "timing": {
    "reset": {"path": "/project1/fade", "parameter": "Reset"},
    "initializeFrames": 1,
    "sampleFrames": [0, 30, 60],
    "after": "pause"
  },
  "inspect": {"paths": ["/project1/fade"], "include": ["params", "errors", "warnings"]}
}
```

`capture` returns `jobId`. Poll `capture` with `action:"status"`, the same
`pid`/`daemonId`, and `jobId`; samples carry offsets, timing, images and optional
inspection. They advance locally between calls: polling does not step TD.
`repeat:{"count":3,"interval":30}` is equivalent to offsets 0/30/60.
`stepFrames:1` advances once then captures. Use only one schedule form.
`timing:{"after":"pause"}` captures and stays paused; `after:"play"` resumes
from the resulting state. `after:"restore"` restores play flags, not history.

Timed sampling requires one explicit TOP and its effective clock (`timePath`
is optional, inferred from the output). Paired inspect paths share that clock;
`content` is excluded because it can force shader compilation. The runner
cooks every intervening output frame, including between sparse samples.
Timeline-range crossings are rejected; extend the range explicitly beforehand.

## Job ownership and failures

While a job is running/finalizing, only its status/cancel calls and heartbeat
are admitted on that PID. `tdmcp.timing.busy` names the owning tool/job. Do not
use execute_python or mutations to bypass it. Status/cancel without a jobId
can recover the active job after a lost start reply. RPC timeout is not cancel.

Use `action:"cancel"`, then poll until cleanup is terminal. `cleanup_failed`
still owns the PID: inspect the returned error and retry cancel after resolving
the cleanup problem. External play/frame/rate/source changes, deadline and
disconnect stop advancement; failures leave playback paused. Never label
`ok:true` on a status call as job success: check `state`, `error`, actual
progress and artifact metadata. Bridge reload invalidates jobs/artifacts.

## Record and retrieve

Call `record` with `pid`, explicit TOP `path`, `frames:N`, and optional `timing`
reset/initialization/after options. N means offsets 0 through N−1, not N advances.
Video has no audio; the initial backend is qtrle MOV at the effective timeline
FPS. Controlled frame stepping does not measure real-time performance.

Poll `record action:"status"`. After successful finalization, `artifact`
reports container-verified frame count, dimensions, FPS, bytes and artifactId.
Retrieve with `record action:"read"`, jobId, byte `offset`, and `length` up to
262144. Decode `dataBase64`, advance to `nextOffset`, and stop at `eof:true`.
This works through federation without accessing a remote filesystem path.
For visual claims, decode/view the resulting video; container validation alone
does not establish intended pixels. Cancellation may retain a partial artifact.

Bounds: 16 capture samples / 8 MiB aggregate results; 3600 advances or video
frames; 512 MiB video; 1–600 second job deadline. At most eight jobs are retained
per bridge generation. Download promptly; `release` deletes retained results
and private files, and oldest results can be evicted by later jobs.

## Related

- {{ skill("play-state") }}
- {{ skill("look-grade") }}
- {{ skill("tooling-concurrency") }}

---

**Canonical:** {{ skill("timed-capture") }}
