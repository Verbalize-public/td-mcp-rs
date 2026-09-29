"""Loaded bridge identity and bounded route capability metadata.

The ready payload is built once, after the daemon-selected package has been
reloaded.  It deliberately keeps the loaded-code fingerprint separate from a
snapshot of source bytes found on disk at that moment: neither identity is
presented as the other, and later disk changes do not rewrite load history.
"""
from __future__ import annotations

import hashlib
import marshal
import os
import platform
import sys
import uuid
from types import CodeType, ModuleType
from typing import Any

from .constants import __protocol_version__
from . import state as _state

# These are exactly the request routes implemented by HANDLERS.  Capability
# support must never be inferred from a package version or proxy catalogue.
SUPPORTED_FEATURES: tuple[str, ...] = (
    "execute_python",
    "capture",
    "sample_data",
    "script_source",
    "record",
    "inspect",
    "mutate_nodes",
    "api_help",
    "editor_context",
    "palette_probe",
    "ping",
)

_LOAD_GENERATION = str(uuid.uuid4())
_READY_PAYLOAD: dict[str, Any] | None = None

# Selected loaded callables cover every advertised route plus the two serving
# paths.  The fingerprint is intentionally bounded rather than claiming the
# entire Python/native dependency closure.
_CODE_BINDINGS: tuple[tuple[str, str, str], ...] = (
    ("execute_python", "tdmcp_bridge.execute", "handle_execute_python"),
    ("capture", "tdmcp_bridge.capture", "handle_capture"),
    ("sample_data", "tdmcp_bridge.sample_data", "handle_sample_data"),
    ("script_source", "tdmcp_bridge.script_source", "handle_script_source"),
    ("record", "tdmcp_bridge.timing", "handle"),
    ("inspect", "tdmcp_bridge.inspect", "handle_inspect"),
    ("mutate_nodes", "tdmcp_bridge.mutate", "handle_mutate"),
    ("api_help", "tdmcp_bridge.api_help", "handle_api_help"),
    ("editor_context", "tdmcp_bridge.editor_context", "handle_editor_context"),
    ("palette_probe", "tdmcp_bridge.palette", "handle_palette_probe"),
    ("dispatch", "tdmcp_bridge", "dispatch"),
    ("serve", "tdmcp_bridge.task_queue", "serve"),
    ("serve_queued", "tdmcp_bridge.task_queue", "serve_queued"),
)


def _callable_code(value: Any) -> CodeType | None:
    code = getattr(value, "__code__", None)
    return code if isinstance(code, CodeType) else None


def _loaded_code_identity() -> dict[str, Any]:
    digest = hashlib.sha256()
    covered: list[str] = []
    missing: list[str] = []
    for label, module_name, attr_name in _CODE_BINDINGS:
        module = sys.modules.get(module_name)
        value = getattr(module, attr_name, None) if module is not None else None
        code = _callable_code(value)
        qualified = f"{module_name}.{attr_name}"
        if code is None:
            missing.append(qualified)
            continue
        encoded = marshal.dumps(code)
        name = qualified.encode("utf-8")
        digest.update(len(name).to_bytes(4, "little"))
        digest.update(name)
        digest.update(len(encoded).to_bytes(8, "little"))
        digest.update(encoded)
        covered.append(label)
    return {
        "kind": "pythonCodeObjects",
        "algorithm": "sha256-python-marshal-v1",
        "digest": digest.hexdigest(),
        "coverage": covered,
        "missing": missing,
        "limitations": [
            "covers selected loaded Python route/transport code objects only",
            "does not cover native extensions, runtime imports, or the full dependency closure",
        ],
    }


def _source_modules() -> list[ModuleType]:
    modules: list[ModuleType] = []
    for name, module in sorted(sys.modules.items()):
        if name == "tdmcp_bridge" or name.startswith("tdmcp_bridge."):
            if isinstance(module, ModuleType):
                modules.append(module)
    return modules


def _source_snapshot() -> dict[str, Any]:
    digest = hashlib.sha256()
    files: list[str] = []
    unavailable: list[str] = []
    package = sys.modules.get("tdmcp_bridge")
    package_file = getattr(package, "__file__", None)
    package_root = os.path.dirname(package_file) if isinstance(package_file, str) else None
    seen: set[str] = set()
    for module in _source_modules():
        path = getattr(module, "__file__", None)
        if not isinstance(path, str) or not path.endswith(".py"):
            continue
        canonical = os.path.realpath(path)
        if canonical in seen:
            continue
        seen.add(canonical)
        relative = os.path.relpath(canonical, package_root) if package_root else canonical
        relative = relative.replace(os.sep, "/")
        try:
            with open(canonical, "rb") as source:  # noqa: PTH123 - loaded module path
                body = source.read()
        except OSError:
            unavailable.append(relative)
            continue
        name = relative.encode("utf-8")
        digest.update(len(name).to_bytes(4, "little"))
        digest.update(name)
        digest.update(len(body).to_bytes(8, "little"))
        digest.update(body)
        files.append(relative)
    return {
        "kind": "sourceSnapshot",
        "provenance": "onDiskAtReadyAfterFinalReload",
        "algorithm": "sha256-framed-files-v1",
        "digest": digest.hexdigest(),
        "files": files,
        "unavailable": unavailable,
        "limitations": [
            "records source bytes observed at readiness, not later on-disk state",
            "source identity is distinct from the loaded Python code-object identity",
            "does not cover native extensions or files outside loaded tdmcp_bridge modules",
        ],
    }


def _bundle_snapshot() -> dict[str, Any]:
    """Fingerprint the daemon-selected Python bundle as observed at readiness."""
    package = sys.modules.get("tdmcp_bridge")
    package_file = getattr(package, "__file__", None)
    package_root = os.path.dirname(package_file) if isinstance(package_file, str) else None
    bridge_root = os.path.dirname(package_root) if package_root else None
    candidates: list[str] = []
    if package_root:
        for root, dirs, names in os.walk(package_root):
            dirs[:] = sorted(name for name in dirs if name != "__pycache__")
            candidates.extend(
                os.path.join(root, name) for name in sorted(names) if name.endswith(".py")
            )
    if bridge_root:
        candidates.append(os.path.join(bridge_root, "manifest.json"))

    digest = hashlib.sha256()
    files: list[str] = []
    unavailable: list[str] = []
    for path in sorted(set(candidates)):
        relative = os.path.relpath(path, bridge_root).replace(os.sep, "/") if bridge_root else path
        try:
            with open(path, "rb") as source:  # noqa: PTH123 - selected package path
                body = source.read()
        except OSError:
            unavailable.append(relative)
            continue
        name = relative.encode("utf-8")
        digest.update(len(name).to_bytes(4, "little"))
        digest.update(name)
        digest.update(len(body).to_bytes(8, "little"))
        digest.update(body)
        files.append(relative)
    return {
        "kind": "bridgePackageSnapshot",
        "provenance": "daemonSelectedPackageOnDiskAtReadyAfterFinalReload",
        "algorithm": "sha256-framed-files-v1",
        "digest": digest.hexdigest(),
        "files": files,
        "unavailable": unavailable,
        "limitations": [
            "records manifest.json and tdmcp_bridge Python source observed at readiness",
            "does not claim later on-disk state or identify the loaded native dependency closure",
            "bootstrap.py and tox_callbacks.py are bootstrap assets, not part of this loaded package snapshot",
        ],
    }


def ready_payload() -> dict[str, Any]:
    """Return immutable metadata for this final package load generation."""
    global _READY_PAYLOAD
    if _READY_PAYLOAD is None:
        _READY_PAYLOAD = {
            "loadGeneration": _LOAD_GENERATION,
            "protocol": {
                "name": "tdmcp-bridge-ipc",
                "version": __protocol_version__,
            },
            "runtime": {
                "implementation": platform.python_implementation(),
                "pythonVersion": platform.python_version(),
                "platform": platform.system() or sys.platform,
            },
            "supportedFeatures": list(SUPPORTED_FEATURES),
            "loadedCode": _loaded_code_identity(),
            "sourceSnapshot": _source_snapshot(),
            "bundleSnapshot": _bundle_snapshot(),
            "mainThreadPumpAgeMs": _state.main_thread_pump_age_ms(),
        }
    # Only nested values created above are immutable-by-convention.  Returning
    # a shallow copy keeps callers from replacing top-level cached fields.
    return dict(_READY_PAYLOAD)


def ready_event() -> dict[str, Any]:
    """Build the optional post-reload bridge-ready event frame."""
    return {"type": "event", "name": "bridge_ready", "payload": ready_payload()}


def heartbeat_payload() -> dict[str, Any]:
    """Worker-safe pong metadata without claiming a main-thread observation.

    The worker computes age from the cached timestamp, but only main-thread
    pump code may update that timestamp.
    """
    return {
        "ok": True,
        "pong": True,
        "bridge": {
            "loadGeneration": _LOAD_GENERATION,
            "mainThreadPumpAgeMs": _state.main_thread_pump_age_ms(),
        },
    }


def supports_required_features(required: list[str] | tuple[str, ...]) -> bool:
    """Predicate for future dispatch admission; ordinary legacy calls ignore it."""
    return all(feature in SUPPORTED_FEATURES for feature in required)


def require_supported_features(required: list[str] | tuple[str, ...]) -> None:
    """Reject unsupported future feature requirements before any effects."""
    unsupported = sorted({feature for feature in required if feature not in SUPPORTED_FEATURES})
    if unsupported:
        raise ValueError("unsupported loaded bridge features: " + ", ".join(unsupported))
