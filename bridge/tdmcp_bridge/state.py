"""Helpers to read/write package-level mutable state (monkeypatch-friendly)."""

from __future__ import annotations

import sys
from types import ModuleType


def _pkg() -> ModuleType:
    return sys.modules[__package__]


def get_bridge_host_path() -> str | None:
    return getattr(_pkg(), "_bridge_host_path", None)


def set_bridge_host_path(path: str | None) -> None:
    _pkg()._bridge_host_path = path


def get_capture_depth() -> int:
    return int(getattr(_pkg(), "_capture_depth", 0))


def set_capture_depth(value: int) -> None:
    _pkg()._capture_depth = value
