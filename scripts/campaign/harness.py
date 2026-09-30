#!/usr/bin/env python3
"""Task-owned session lifecycle for a campaign round.

The campaign drives a real TouchDesigner through a real MCP transport, so it
must own every process it touches and leave none behind. Three rules, each
learned the hard way:

Ownership is by pid provenance, never by name. The user has their own daemon
and their own TouchDesigner instances; `pkill -f TouchDesigner` is forbidden
even as a fallback.

The bridge port is `TDMCP_IPC_PORT`, not `--port`. `--port` is the HTTP port;
the default bridge port 9861 belongs to the installed daemon, and a campaign
daemon that tries to bind it dies with `Address already in use (os error 98)`.

Teardown is in a `finally` and is verified, not assumed. A round that leaves
a TouchDesigner running makes the NEXT round's measurement describe the
previous round's debris.
"""
from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable

REPO = Path(__file__).resolve().parents[2]
if str(REPO / "scripts") not in sys.path:
    sys.path.insert(0, str(REPO / "scripts"))

from mcp_probe import McpClient  # noqa: E402  (path set above)
from preflight import touchdesigner_pids  # noqa: E402

# Re-exported so sibling modules can import their transport from one place.
__all__ = ["Session", "McpClient", "BRIDGE_PORT", "MCP_PORT", "REPO"]

BRIDGE_PORT = 9987
MCP_PORT = 9988
DAEMON = REPO / "target" / "release" / "tdmcp-daemon"


def _has_td_cmdline(pid: int) -> bool:
    """Only ever signal a pid whose own command line proves it is TouchDesigner.

    `kill_td` in the daemon refuses a pid it does not know, and a campaign
    must not be in the business of killing things it spawned no record of.
    """
    try:
        cmdline = Path(f"/proc/{pid}/cmdline").read_bytes().decode("utf-8", "replace")
    except OSError:
        return False
    return "TouchDesigner.exe" in cmdline


def _terminate_td(pid: int) -> None:
    """SIGTERM, then SIGKILL, one exact pid that /proc proves is TouchDesigner."""
    if not _has_td_cmdline(pid):
        return
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    deadline = time.monotonic() + 15.0
    while time.monotonic() < deadline:
        if not Path(f"/proc/{pid}").exists():
            return
        time.sleep(0.5)
    if _has_td_cmdline(pid):
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            return
        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline and Path(f"/proc/{pid}").exists():
            time.sleep(0.5)


def _unpack_refusal(exc: RuntimeError) -> dict:
    """Turn an `isError` exception back into the payload it carried.

    `McpClient.call` raises `RuntimeError(<the whole result dict>)`, so the
    diagnostic survives in the exception's args. Where the structured content
    is there, it is returned verbatim -- reshaping a refusal would destroy the
    distinctions this campaign exists to check. Where it is not, the raw dict
    is returned so the caller still sees `isError:true` rather than an opaque
    string.
    """
    args = exc.args
    raw = args[0] if args else None
    if isinstance(raw, dict):
        structured = raw.get("structuredContent")
        if isinstance(structured, dict):
            return structured
        return raw
    return {"ok": False, "summary": f"transport refusal: {raw!r}"[:400]}


@dataclass
class Session:
    """A task-owned daemon plus, optionally, one TouchDesigner."""

    data_dir: Path
    run_dir: Path
    daemon_proc: subprocess.Popen | None = None
    td_pid: int | None = None
    #: Additional TouchDesigners this round owns (a spawn that timed out can
    #: leave more than one running); all of them are stopped in teardown.
    _extra_td_pids: list[int] = field(default_factory=list)
    client: McpClient | None = None
    log_lines: list[str] = field(default_factory=list)
    #: Lines the daemon's child processes (TouchDesigner, Wine) wrote to the
    #: same pipe. Kept apart from `log_lines` so a failure is diagnosable:
    #: `GLXBadDrawable` in here is evidence about the child's display, not a
    #: daemon error, and reading it as one would misattribute the fault.
    log_noise: list[str] = field(default_factory=list)
    _drain: threading.Thread | None = None

    # -- daemon ------------------------------------------------------------

    def start_daemon(self, *, timeout_s: float = 30.0) -> None:
        """Start the campaign daemon and wait for its HTTP port to answer."""
        env = dict(os.environ)
        env["TDMCP_IPC_PORT"] = str(BRIDGE_PORT)  # the BRIDGE port, not --port
        env["TDMCP_NO_GUI"] = "true"  # a tray in a headless campaign is noise
        env.pop("TDMCP_BRIDGE_DIR", None)  # force the handshake's own answer
        self.data_dir.mkdir(parents=True, exist_ok=True)
        self.daemon_proc = subprocess.Popen(
            [
                str(DAEMON), "start",
                "--port", str(MCP_PORT),
                "--data-dir", str(self.data_dir),
                # The CLI flag is a bare `--no-gui` (clap SetTrue takes no
                # value), while the env var is parsed as a bool that only
                # accepts the literal strings "true"/"false" -- passing "1"
                # for either is a startup failure, not a preference.
                "--no-gui",
            ],
            cwd=str(REPO),
            env=env,
            # Drained on a thread, not inherited and not blocked on: the daemon
            # LAUNCHES TouchDesigner, and TouchDesigner under Wine writes a
            # running commentary to stdout (dwrite/d2d/winsock fixmes). Left
            # undrained the 64 KiB pipe buffer fills and BLOCKS THE DAEMON
            # mid-write; inherited, the commentary lands in the campaign's own
            # output and buries the verdicts. A measured case: the pilot's log
            # tail was 25 lines of Wine fixmes and no daemon lines.
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        self._drain = threading.Thread(
            target=self._drain_stdout, args=(self.daemon_proc,), daemon=True
        )
        self._drain.start()

        url = f"http://127.0.0.1:{MCP_PORT}"
        deadline = time.monotonic() + timeout_s
        last_error = "never attempted"
        while time.monotonic() < deadline:
            if self.daemon_proc.poll() is not None:
                raise RuntimeError(
                    f"campaign daemon exited rc={self.daemon_proc.returncode}: "
                    f"{''.join(self.log_lines)[-800:]}"
                )
            try:
                probe = McpClient(base=url)
                probe.close()
                self.client = McpClient(base=url)
                return
            except Exception as exc:  # not up yet; keep waiting
                last_error = f"{type(exc).__name__}: {exc}"
                time.sleep(0.5)
        self.stop_daemon()
        raise RuntimeError(f"campaign daemon did not answer on {url}: {last_error}")

    def _drain_stdout(self, proc: subprocess.Popen) -> None:
        """Keep ONLY the daemon's own JSON log lines.

        The daemon and the TouchDesigner it launched share this pipe, and
        TouchDesigner under Wine writes a running commentary to it (dwrite,
        d2d and GLX fixmes). A measured case: the tail was 25 lines of Wine
        noise and not one daemon line, which is the opposite of what a failed
        run needs. A line is kept only when it parses as a JSON object
        carrying the `seq` field every daemon log line has -- anything else,
        including a startup error printed before the logger exists, is kept
        separately under `noise` so it is still recorded, just not mistaken
        for the daemon's account of the run.
        """
        if proc.stdout is None:
            return
        for line in proc.stdout:
            line = line.rstrip("\n")
            stripped = line.strip()
            try:
                record = json.loads(stripped) if stripped.startswith("{") else None
            except ValueError:
                record = None
            if isinstance(record, dict) and "seq" in record:
                self.log_lines.append(line)
                if len(self.log_lines) > 400:
                    del self.log_lines[:100]
            elif stripped:
                self.log_noise.append(stripped)
                if len(self.log_noise) > 200:
                    del self.log_noise[:50]

    def stop_daemon(self) -> None:
        if self.daemon_proc is None:
            return
        if self.daemon_proc.poll() is None:
            self.daemon_proc.terminate()
            try:
                self.daemon_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.daemon_proc.kill()
                self.daemon_proc.wait(timeout=5)
        # Join the reader before reading, so the tail is complete rather than
        # whatever happened to have been flushed.
        drain = getattr(self, "_drain", None)
        if drain is not None:
            drain.join(timeout=5)
        self.daemon_proc = None

    # -- mcp ---------------------------------------------------------------

    @property
    def call(self) -> Callable[[str, dict, int], dict]:
        """Call a tool and ALWAYS return a payload, never raise on a refusal.

        `McpClient.call` raises `RuntimeError` on `isError`, which is right for
        a script that wants a non-zero exit and wrong here: a refusal is one of
        the things under test. The diagnostic payload is unpacked so the
        predicates can read `ok`, `summary` and the per-step codes -- raising
        would turn every guarded-write refusal into a scenario error and the
        most interesting behaviour in the product into a test that never ran.
        """
        if self.client is None:
            raise RuntimeError("session has no MCP client; start_daemon first")
        client = self.client

        def _call(tool: str, args: dict, timeout_s: int = 120) -> dict:
            try:
                return client.call(tool, args)
            except RuntimeError as exc:
                return _unpack_refusal(exc)

        return _call

    def read_resource(self, uri: str) -> Any:
        if self.client is None:
            raise RuntimeError("session has no MCP client")
        return self.client.request("resources/read", {"uri": uri})

    # -- touchdesigner -----------------------------------------------------

    def spawn_td(self, *, timeout_s: float = 180.0) -> int:
        """Spawn the Pro TouchDesigner and wait for the bridge handshake.

        The handshake is the whole point: a TD that opens without one is not
        a usable session, and round 36 measured a TD that opened, handshaked,
        and then died on a heartbeat timeout. Both must be visible, which is
        why the pid is only returned after the bridge reports connected.

        When the reply does not arrive, the spawn is STILL OWNED. A measured
        `wait_timeout` leaves a live TouchDesigner running that the reply
        never named, and a harness that only cleans up pids it was told about
        orphans it -- the next round then measures the previous round's
        debris. So the daemon's own fleet is consulted and any TouchDesigner
        that appeared during the call is adopted before the failure is
        reported.
        """
        from scenario import Refusal  # local import: keeps the module import-light

        before = set(touchdesigner_pids())
        payload = self.call("spawn_td", {}, timeout_s)
        if _extract_pid(payload) is None:
            self._adopt_unreported_td(before)
            raise Refusal({"ok": False, "summary": f"spawn_td returned no pid: {json.dumps(payload)[:400]}"})
        pid = _extract_pid(payload)
        self.td_pid = pid

        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            try:
                fleet = self.call("fleet", {"pids": [pid]}, 30)
            except Exception:
                time.sleep(2.0)
                continue
            if _bridge_connected(fleet, pid):
                return pid
            time.sleep(2.0)
        raise RuntimeError(
            f"TD pid {pid} did not report a connected bridge within {timeout_s:.0f}s"
        )

    def _adopt_unreported_td(self, before: set[int]) -> list[int]:
        """Take ownership of a TouchDesigner the spawn started but never named.

        Records the pid so teardown stops it, and prefers the daemon's fleet
        row over a process scan: the fleet knows which instances carry
        `spawn` provenance, so it distinguishes a TouchDesigner this round
        started from one the user opened in the same window.
        """
        appeared = sorted(set(touchdesigner_pids()) - before)
        if not appeared:
            return []
        owned: list[int] = []
        try:
            fleet = self.call("fleet", {"include": ["process"]}, 30)
            rows = _dig(fleet, "processes")
            for row in rows if isinstance(rows, list) else []:
                if not isinstance(row, dict) or row.get("pid") not in appeared:
                    continue
                if row.get("spawn"):
                    owned.append(int(row["pid"]))
        except Exception:
            pass
        if not owned:
            # The fleet could not answer, so provenance is unknown. Adopting
            # every new instance is the conservative direction: leaving a
            # TouchDesigner running is worse than reporting one this round did
            # not in fact start, and the record says which choice was made.
            owned = appeared
            self.log_lines.append(
                f"campaign: adopted {appeared} without fleet provenance (fleet did not answer)"
            )
        # The first adopted pid becomes THE session pid so scenarios can use
        # it; the rest are still owned so teardown stops them.
        self.td_pid = owned[0]
        self._extra_td_pids = [p for p in owned[1:]]
        self.log_lines.append(f"campaign: adopted unreported TouchDesigner pids {owned}")
        return owned

    def stop_td(self) -> None:
        """Terminate every task-owned TouchDesigner by exact pid, after proving it.

        The pid is cleared only AFTER the process is actually gone, because
        teardown's leftover check asks "does this session still own something",
        and a session that forgets its pid on the first attempt would report
        'clean' while a TouchDesigner it started kept running -- exactly the
        debris that makes the next round's measurement describe the previous
        one. A survivor is therefore KEPT, in `td_pid` when it is the session
        pid and in `_extra_td_pids` otherwise, so the leftover report names it.
        """
        pids = list(dict.fromkeys([p for p in [self.td_pid, *self._extra_td_pids] if p]))
        for pid in pids:
            _terminate_td(pid)
        survivors = [p for p in pids if Path(f"/proc/{p}").exists()]
        self.td_pid = self.td_pid if self.td_pid in survivors else None
        self._extra_td_pids = [p for p in self._extra_td_pids if p in survivors]

    def close(self) -> None:
        if self.client is not None:
            self.client.close()
            self.client = None

    def teardown(self) -> dict:
        """Return every resource, in order, and report what is actually left."""
        report: dict[str, Any] = {}
        try:
            self.stop_td()
            report["td_stopped"] = True
        except Exception as exc:
            report["td_stopped"] = f"error: {exc}"
        self.close()
        self.stop_daemon()
        leftovers = []
        # Every owned pid, not just the session one: a second adopted
        # TouchDesigner that survives is exactly as much debris as the first,
        # and reading only `td_pid` is how a multi-spawn run reported clean
        # with a process still running.
        for pid in dict.fromkeys([p for p in [self.td_pid, *self._extra_td_pids] if p]):
            leftovers.append(f"td:{pid}")
        if self.daemon_proc is not None and self.daemon_proc.poll() is None:
            leftovers.append(f"daemon:{self.daemon_proc.pid}")
        report["leftovers"] = leftovers
        return report


def _extract_pid(payload: Any) -> int | None:
    from scenario import _dig  # local import

    for path in ("pid", "result.pid", "items.0.result.pid", "items.0.result.process.pid"):
        value = _dig(payload, path)
        if isinstance(value, int) and value > 0:
            return value
    return None


def _bridge_connected(fleet: Any, pid: int) -> bool:
    """Did the fleet report a live, connected bridge for THIS pid?

    `fleet` answers `{"processes": [...]}`, a LIST of rows keyed by `pid`, and
    the bridge state is a bare string (`"starting"`, `"connected"`, ...), not a
    nested object. Matching on the row's own pid matters: with any other
    instance on the host, "some bridge is connected" is not evidence that
    *this* session is up, and a run that accepts it would be measuring
    somebody else's TouchDesigner.
    """
    from scenario import _dig

    rows = _dig(fleet, "processes")
    if not isinstance(rows, list):
        return False
    for row in rows:
        if not isinstance(row, dict) or row.get("pid") != pid:
            continue
        bridge = row.get("bridge")
        if isinstance(bridge, str):
            return bridge.lower() in ("connected", "ready", "ok")
        if isinstance(bridge, dict):
            return str(bridge.get("state", "")).lower() in ("connected", "ready", "ok")
        return str(row.get("bridgeState", "")).lower() in ("connected", "ready", "ok")
    return False
