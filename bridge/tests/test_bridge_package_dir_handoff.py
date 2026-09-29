"""The handshake must decide which bridge package TD runs, not `sys.path` order.

`bootstrap_threaded` resolves the package directory with an explicit precedence
(override -> env -> handshake `bridgePackageDir` -> conventional) and then calls
`_load_bridge_package(pkg_dir)`. If that call only put `pkg_dir` on `sys.path`
and reloaded, the resolution would be decorative: `importlib.reload` re-executes
a module from its EXISTING `__file__`, it does not re-resolve the import
through `sys.path`. The first copy `bootstrap.py` imported would keep running
forever, and the daemon would be serving `bridgePackageDir` to a session that
never loads it.

These tests are about which FILE the running module came from, because that is
the only thing a TD session can observe. They build two real package trees on
disk, differ only in a marker constant, and assert which one is live after a
load. No `__file__` stubbing, no mocking of the loader: the defect, if there is
one, lives in real import semantics.
"""
from __future__ import annotations

import importlib
import json
import os
import sys
import textwrap
from pathlib import Path

import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import tdmcp_bridge  # noqa: E402 — the path shim above must run first


def _package_tree(root: Path, marker: int) -> Path:
    """Write a minimal but REAL `tdmcp_bridge` package carrying `marker`."""
    package = root / "tdmcp_bridge"
    package.mkdir(parents=True)
    (package / "__init__.py").write_text(
        textwrap.dedent(
            f'''\
            """Minimal stand-in package; MARKER is what the test reads."""
            MARKER = {marker}

            def reload_marker() -> int:
                return MARKER
            '''
        ),
        encoding="utf-8",
    )
    return root


@pytest.fixture
def two_trees(tmp_path):
    """Two complete package trees, and a guaranteed restore of interpreter state.

    `sys.modules` and `sys.path` are process-wide, so every failure path here
    restores them; a leaked entry would silently steer a later test that imports
    `tdmcp_bridge`.
    """
    old_modules = {k: v for k, v in sys.modules.items() if k.startswith("tdmcp_bridge")}
    old_path = list(sys.path)
    first = _package_tree(tmp_path / "first", 1)
    second = _package_tree(tmp_path / "second", 2)
    for key in [k for k in sys.modules if k.startswith("tdmcp_bridge")]:
        del sys.modules[key]
    try:
        yield first, second
    finally:
        sys.path[:] = old_path
        for key in [k for k in sys.modules if k.startswith("tdmcp_bridge")]:
            del sys.modules[key]
        sys.modules.update(old_modules)


def _load(target: Path):
    """Run the real loader the bootstrap calls, in the order the bootstrap does.

    `bootstrap.py` imports `tdmcp_bridge` from whichever tree resolves first,
    then `_load_bridge_package` re-points the session. Both orderings matter, so
    the helper reproduces the production one rather than a tidier invented one.
    """
    sys.path.insert(0, str(target))
    importlib.import_module("tdmcp_bridge")
    tdmcp_bridge._load_bridge_package(str(target))
    return importlib.import_module("tdmcp_bridge")


def test_loading_a_second_tree_replaces_the_first_on_disk(two_trees):
    """The handshake dir must become the LIVE package, not merely precede it."""
    first, second = two_trees

    assert _load(first).reload_marker() == 1
    # The interesting case: a different directory, exactly what a daemon
    # restart on a new data dir, or a `--bridge-dir` override, produces.
    assert _load(second).reload_marker() == 2, (
        "the package still running is the FIRST tree: `_load_bridge_package` "
        "put the new dir on sys.path but `importlib.reload` re-executed the "
        "already-imported module from its old __file__, so the bridge TD runs "
        "is not the bridge the daemon handed over"
    )


def test_loading_the_same_tree_twice_is_stable(two_trees):
    """A reload of the live tree still works (the ordinary reconnect path)."""
    first, _ = two_trees

    assert _load(first).reload_marker() == 1
    assert _load(first).reload_marker() == 1


def test_the_handshake_dir_beats_the_pre_handshake_import_dir(two_trees, monkeypatch):
    """`bootstrap_threaded` loads a PRE-handshake dir, then the handshake one.

    This is the production order, and it is the one that decides the round-30
    question: if the earlier dir keeps winning on `sys.path`, the session keeps
    running the copy `bootstrap.py` happened to import, and the daemon's
    `bridgePackageDir` is served to a session that never loaded it.
    """
    imported_first, handshake = two_trees
    monkeypatch.setattr(
        tdmcp_bridge, "_resolve_bridge_package_dir", lambda _b, _r: str(handshake)
    )

    pre_dir = imported_first
    sys.path.insert(0, str(pre_dir))
    importlib.import_module("tdmcp_bridge")
    assert importlib.import_module("tdmcp_bridge").reload_marker() == 1

    # Exactly the bootstrap sequence: resolve, then re-point the live session.
    pkg_dir = tdmcp_bridge._resolve_bridge_package_dir(None, {})
    tdmcp_bridge._load_bridge_package(pkg_dir)
    assert importlib.import_module("tdmcp_bridge").reload_marker() == 2, (
        "the live package is still the pre-handshake import dir, so the "
        "bridge TD runs is not the bridge the daemon handed over"
    )


def test_the_resolution_order_prefers_the_handshake_directory(two_trees, monkeypatch):
    """`bridgePackageDir` is what makes the precedence meaningful at all."""
    _, second = two_trees
    monkeypatch.setattr(tdmcp_bridge, "_env_bridge_dir", lambda: None)
    monkeypatch.setattr(tdmcp_bridge, "_conventional_bridge_dir", lambda: None)

    response = json.loads(json.dumps({"bridgePackageDir": str(second)}))
    assert tdmcp_bridge._resolve_bridge_package_dir(None, response) == str(second)
