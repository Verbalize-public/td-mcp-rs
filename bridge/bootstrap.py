"""td-mcp-rs bootstrap — drop into a Text DAT inside a TouchDesigner project.

Dials the local td-mcp-rs daemon over named pipe (Windows) / UDS (Unix),
performs the handshake, loads the ``tdmcp_bridge`` package, and starts the
framed read loop **on a background thread** so TD's main thread keeps
cooking. The shipped ``.tox`` wraps this script in a Text DAT run once by an
Execute DAT's ``onStart``.

That same Execute DAT **must** also have ``Frame Start`` enabled and call
``tdmcp_bridge.process_pending()`` from its ``onFrameStart`` — the worker
thread only enqueues requests (never touches `td.*`); the per-frame pump on
the main thread is what actually dispatches them. Without it, every request
blocks the daemon side forever waiting on a response that's never produced.
Minimal companion `onFrameStart`::

    def onFrameStart(frame):
        try:
            import tdmcp_bridge
            tdmcp_bridge.process_pending()
        except Exception as e:
            print("tdmcp bridge pump:", e)
        return

IMPORTANT: this script is exec'd as a Text DAT's contents, not run from a
file on disk — ``__file__`` is not meaningful here, so the bridge package
directory is resolved from (in order):
  1. ``TDMCP_BRIDGE_DIR`` env var,
  2. the OS-conventional daemon data dir (``%LOCALAPPDATA%/tdmcp-rs/bridge``
     on Windows, ``~/.local/share/tdmcp-rs/bridge`` on Linux/macOS),
  3. the daemon's handshake response (advisory; reload from disk each connect
     if the above two are absent/stale).
"""

from __future__ import annotations

import os
import sys


def _default_data_dir() -> str:
    if sys.platform.startswith("win"):
        base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~")
        return os.path.join(base, "tdmcp-rs")
    if sys.platform == "darwin":
        return os.path.join(
            os.path.expanduser("~"), "Library", "Application Support", "tdmcp-rs"
        )
    base = os.environ.get("XDG_DATA_HOME") or os.path.join(
        os.path.expanduser("~"), ".local", "share"
    )
    return os.path.join(base, "tdmcp-rs")


def _resolve_bridge_dir() -> str | None:
    env = os.environ.get("TDMCP_BRIDGE_DIR")
    if env and os.path.isfile(os.path.join(env, "tdmcp_bridge", "__init__.py")):
        return env
    candidate = os.path.join(_default_data_dir(), "bridge")
    if os.path.isfile(os.path.join(candidate, "tdmcp_bridge", "__init__.py")):
        return candidate
    return None


def main() -> None:
    """Bootstrap the bridge without blocking TD's main thread.

    Safe to call repeatedly (e.g. re-pulsed by an Execute DAT): each call
    dials a fresh connection and spawns a new worker thread. Never raises —
    this runs at project load, so a daemon that isn't up yet should print a
    one-line note in the textport, not throw a scary traceback.
    """
    bridge_dir = _resolve_bridge_dir()
    if bridge_dir and bridge_dir not in sys.path:
        sys.path.insert(0, bridge_dir)
    try:
        import tdmcp_bridge  # noqa: E402  (path set above)

        tdmcp_bridge.bootstrap_threaded(bridge_dir=bridge_dir)
    except Exception as exc:  # noqa: BLE001 — never crash the caller
        print(f"tdmcp-rs bootstrap: could not connect to daemon ({exc})")


if __name__ == "__main__":
    main()
