"""td-mcp-rs bootstrap — drop into a Text DAT inside a TouchDesigner project.

Dials the local td-mcp-rs daemon over named pipe (Windows) / UDS (Unix),
performs the handshake, loads the ``tdmcp_bridge`` package, and runs the framed
dispatch loop. The shipped ``.tox`` wraps this script in a Text DAT whose
``Execute`` DAT pulses it on project load.

The bridge package directory is resolved from (in order):
  1. ``TDMCP_BRIDGE_DIR`` env var,
  2. a ``tdmcp_bridge/`` package beside this script,
  3. the daemon's handshake response (advisory; reload from disk each connect).
"""

from __future__ import annotations

import os
import sys


def _resolve_bridge_dir() -> str | None:
    env = os.environ.get("TDMCP_BRIDGE_DIR")
    if env:
        return env
    here = os.path.dirname(os.path.abspath(__file__))
    for candidate in (here, os.path.join(here, "bridge"), os.path.dirname(here)):
        if os.path.isfile(os.path.join(candidate, "tdmcp_bridge", "__init__.py")):
            return candidate
    return None


def main() -> None:
    bridge_dir = _resolve_bridge_dir()
    if bridge_dir and bridge_dir not in sys.path:
        sys.path.insert(0, bridge_dir)
    import tdmcp_bridge  # noqa: E402  (path set above)

    tdmcp_bridge.bootstrap(bridge_dir=bridge_dir)


if __name__ == "__main__":
    main()
