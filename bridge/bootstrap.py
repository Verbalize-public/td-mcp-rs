"""td-mcp-rs bootstrap — Text DAT body inside the bootstrap ``.tox``.

Dials the local td-mcp-rs daemon over named pipe (Windows) / UDS (Unix),
performs the handshake, loads the ``tdmcp_bridge`` package from the path the
daemon returns, and starts the framed read loop **on a background thread**
so TD's main thread keeps cooking.

The shipped ``.tox`` wraps this script in a Text DAT run by the Execute DAT
callbacks in ``tox_callbacks.py`` (``onStart`` / reconnect). While playing,
that Execute DAT should keep ``Frame Start`` enabled and call
``tdmcp_bridge.process_pending()`` (larger batch). A ``td.run`` pump started
by ``bootstrap_threaded`` also drains the queue while paused — see
``tox_callbacks.py`` (pause-resilient pump design).

IMPORTANT: this script is exec'd as a Text DAT's contents, not run from a
file on disk — ``__file__`` is not meaningful here. Bridge package directory
resolution (inside ``tdmcp_bridge.bootstrap_threaded``):

  1. ``TDMCP_BRIDGE_DIR`` env var (override),
  2. daemon handshake ``bridgePackageDir``,
  3. OS-conventional data dir ``…/tdmcp-rs/bridge``.

This dialer only needs a path to *import* ``tdmcp_bridge`` (env or
conventional); the handshake path wins for the live session unless the env
override is set.
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


def _import_bridge_dir() -> str | None:
    """Path used only to import the package before handshake."""
    env = os.environ.get("TDMCP_BRIDGE_DIR")
    if env and os.path.isfile(os.path.join(env, "tdmcp_bridge", "__init__.py")):
        return env
    candidate = os.path.join(_default_data_dir(), "bridge")
    if os.path.isfile(os.path.join(candidate, "tdmcp_bridge", "__init__.py")):
        return candidate
    return None


def _env_override() -> str | None:
    env = os.environ.get("TDMCP_BRIDGE_DIR")
    if env and os.path.isfile(os.path.join(env, "tdmcp_bridge", "__init__.py")):
        return env
    return None


def main() -> None:
    """Bootstrap the bridge without blocking TD's main thread.

    Safe to call repeatedly (e.g. re-pulsed by an Execute DAT): each call
    dials a fresh connection and spawns a new worker thread. Never raises —
    this runs at project load, so a daemon that isn't up yet should print a
    one-line note in the textport, not throw a scary traceback.
    """
    import_dir = _import_bridge_dir()
    if import_dir and import_dir not in sys.path:
        sys.path.insert(0, import_dir)
    # TD keeps one interpreter across dialer retries — drop stale package
    # modules so disk fixes (e.g. after ensure) are visible immediately.
    for _name in list(sys.modules):
        if _name == "tdmcp_bridge" or _name.startswith("tdmcp_bridge."):
            del sys.modules[_name]
    try:
        import tdmcp_bridge  # noqa: E402  (path set above)

        tdmcp_bridge.bootstrap_threaded(bridge_dir=_env_override())
    except Exception as exc:  # noqa: BLE001 — never crash the caller
        print(f"tdmcp-rs bootstrap: could not connect to daemon ({exc})")


if __name__ == "__main__":
    main()
