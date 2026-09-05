"""Wire framing + TCP loopback transport."""
from __future__ import annotations

import json
import os
import socket
import struct
import time
from typing import Any

from .constants import IDLE_DEAD_S, MAX_FRAME_BYTES, MAX_JSON_DEPTH


class FrameTooLarge(ValueError):
    """A JSON body exceeds the shared daemon/bridge frame budget."""


def _reject_json_constant(value: str) -> None:
    raise ValueError(f"not a JSON number: {value}")

class MidFrameTimeout(TimeoutError):
    """No byte progress while a frame was already partially consumed.

    Distinct from a clean idle ``TimeoutError`` (zero bytes at a frame
    boundary). A single short read-poll stall mid-transfer is **not** fatal —
    streams retry until ``IDLE_DEAD_S`` of silence since the last progress.
    After that, the byte stream is assumed stuck/desynced and
    ``serve_queued`` must disconnect rather than ``continue``.
    """

def _read_frame(stream, *, idle_dead_s: float = IDLE_DEAD_S) -> dict[str, Any]:
    """Read one length-prefixed JSON frame.

    Raises:
        EOFError: peer closed / short read that is not a timeout.
        TimeoutError: underlying stream read timed out (idle poll at frame boundary).
        MidFrameTimeout: no byte progress for ``idle_dead_s`` after the header
            (or mid-body) — stream is stuck/desynced.
        ValueError: oversized frame, invalid JSON, or non-object envelope.
    """
    try:
        header = stream.read(4)
    except MidFrameTimeout:
        raise
    except TimeoutError:
        raise
    if len(header) < 4:
        if len(header) == 0:
            raise EOFError("short header")
        raise EOFError("short header")
    (length,) = struct.unpack("<I", header)
    if length > MAX_FRAME_BYTES:
        # Reject before reading/allocating the body. The caller must close
        # this stream: its unread body cannot be treated as another header.
        raise FrameTooLarge(f"frame exceeds {MAX_FRAME_BYTES} bytes: {length}")
    # Header consumed ⇒ mid-frame even before any body bytes arrive. Tolerate
    # short poll stalls; only die after idle_dead_s with no progress.
    body = bytearray()
    last_progress = time.monotonic()
    while len(body) < length:
        remaining = length - len(body)
        try:
            chunk = stream.read(remaining)
        except MidFrameTimeout:
            raise
        except TimeoutError as exc:
            if idle_dead_s > 0 and (time.monotonic() - last_progress) >= idle_dead_s:
                raise MidFrameTimeout("timed out mid-frame") from exc
            continue
        if not chunk:
            raise EOFError("short body")
        body += chunk
        last_progress = time.monotonic()
    msg = json.loads(body, parse_constant=_reject_json_constant)
    if not isinstance(msg, dict):
        raise ValueError("frame envelope must be a JSON object")
    return msg


def _mid_frame_dead_s(stream, default: float = IDLE_DEAD_S) -> float:
    """Per-stream mid-frame stall budget (set by ``serve_queued``)."""
    value = getattr(stream, "_mid_frame_dead_s", default)
    try:
        return float(value)
    except (TypeError, ValueError):
        return default


def _apply_read_timeout(stream, seconds: float) -> None:
    """Best-effort read timeout for idle polling (the TCP stream wrapper)."""
    setter = getattr(stream, "set_read_timeout", None)
    if callable(setter):
        setter(seconds)

def _check_json_shape(value: Any, depth: int = 0) -> None:
    """Keep Python-only nesting/integers within Rust's JSON representation.

    Recursion is bounded even for a cycle. Integers beyond i64/u64 would
    otherwise silently round to floats in serde_json, or fail to parse.
    """
    if isinstance(value, (dict, list, tuple)):
        if depth >= MAX_JSON_DEPTH:
            raise ValueError(f"JSON nesting exceeds {MAX_JSON_DEPTH} containers")
        for item in value.values() if isinstance(value, dict) else value:
            _check_json_shape(item, depth + 1)
    elif isinstance(value, int) and not -(1 << 63) <= value <= (1 << 64) - 1:
        raise ValueError("JSON integer exceeds i64/u64; return it as a string")


def _encode_body(msg: dict[str, Any]) -> bytearray:
    """Bound the accumulated encoding, including aggregate/escaped text.

    A single encoder chunk (e.g. one string) can still be large; this is a
    wire budget, not a limit on memory used by the handler that made it.
    Never put Python's NaN/Infinity extensions on Rust's strict JSON wire.
    """
    _check_json_shape(msg)
    body = bytearray()
    # UTF-8 is the actual wire encoding. Besides avoiding six-byte escapes
    # for Unicode, encoding here rejects lone surrogates Rust cannot read.
    for chunk in json.JSONEncoder(allow_nan=False, ensure_ascii=False).iterencode(msg):
        encoded = chunk.encode("utf-8")
        if len(body) + len(encoded) > MAX_FRAME_BYTES:
            raise FrameTooLarge(f"response exceeds {MAX_FRAME_BYTES} JSON bytes")
        body.extend(encoded)
    return body


def _write_frame(stream, msg: dict[str, Any]) -> None:
    try:
        body = _encode_body(msg)
    except (TypeError, ValueError, OverflowError, RecursionError) as exc:
        if msg.get("type") != "response":
            raise
        too_large = isinstance(exc, FrameTooLarge)
        # Nothing has been written yet. Replace the unusable response with
        # a correlated error so subsequent calls can use the same stream.
        # The operation already ran: never imply that retrying is safe.
        body = _encode_body({
            "type": "response",
            "id": msg.get("id"),
            "error": {
                "code": "tdmcp.bridge.response_too_large" if too_large else "tdmcp.bridge.response_invalid",
                "message": (
                    f"Bridge response exceeds {MAX_FRAME_BYTES} JSON bytes. " if too_large
                    else "Bridge response cannot be encoded as valid JSON. "
                ) + "The operation may already have run; inspect its state before retrying any mutation.",
            },
        })
    header = struct.pack("<I", len(body))
    n = stream.write(header)
    if n != len(header):
        raise OSError(f"short write: header {n}/{len(header)}")
    n = stream.write(body)
    if n != len(body):
        raise OSError(f"short write: body {n}/{len(body)}")
    stream.flush()

# --- TCP endpoint resolution + dial (docs/LINUX_SUPPORT.md §3, T-8) ---------

_DEFAULT_HOST = "127.0.0.1"
_DEFAULT_PORT = 9861

def _parse_port(raw: str, source: str) -> int:
    """Parse a decimal TCP port (1-65535); ``ValueError`` names ``source``."""
    try:
        port = int(raw)
    except ValueError:
        raise ValueError(f"{source} must be an integer port, got {raw!r}") from None
    if not 1 <= port <= 65535:
        raise ValueError(f"{source} port out of range 1-65535, got {raw!r}")
    return port

def _parse_endpoint(raw: str, source: str) -> tuple[str, int]:
    """Parse a ``host:port`` string; ``ValueError`` names ``source`` on garbage."""
    host, sep, port_s = raw.rpartition(":")
    if not sep or not host:
        raise ValueError(f"{source} must be 'host:port', got {raw!r}")
    return host, _parse_port(port_s, source)

def resolve_endpoint() -> tuple[str, int]:
    """Resolve the daemon TCP endpoint as ``(host, port)``.

    Precedence: ``TDMCP_IPC_ENDPOINT`` (``host:port``) if set, else
    ``TDMCP_IPC_PORT`` (host defaults to 127.0.0.1), else the default
    ``127.0.0.1:9861``. Malformed values raise ``ValueError`` naming the
    offending variable, so a bad env never dials garbage.
    """
    raw = os.environ.get("TDMCP_IPC_ENDPOINT")
    if raw:
        return _parse_endpoint(raw, "TDMCP_IPC_ENDPOINT")
    raw = os.environ.get("TDMCP_IPC_PORT")
    if raw:
        return _DEFAULT_HOST, _parse_port(raw, "TDMCP_IPC_PORT")
    return _DEFAULT_HOST, _DEFAULT_PORT

def dial(endpoint: str | None = None):
    """Connect to the daemon over TCP. Returns a file-like stream.

    ``endpoint`` is an optional ``"host:port"`` override; without it the
    endpoint comes from [`resolve_endpoint`] (env precedence, then the
    ``127.0.0.1:9861`` default).
    """
    if endpoint is None:
        host, port = resolve_endpoint()
    else:
        host, port = _parse_endpoint(endpoint, "endpoint")
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.connect((host, port))
    return _TcpStream(sock)

class _TcpStream:
    """Minimal file-like wrapper over a TCP socket (uniform on every OS).

    Unbuffered by design (manual length-prefixed framing already batches
    reads/writes); avoids ``makefile()``'s buffering surprises and keeps a
    real socket reference for ``shutdown``, which reliably unblocks a
    concurrent blocking ``recv()`` on another thread.
    """

    def __init__(self, sock) -> None:
        self._sock = sock

    def set_read_timeout(self, seconds: float | None) -> None:
        """Socket-level recv timeout for idle polling (``None`` = block forever)."""
        self._sock.settimeout(seconds)

    def read(self, n: int) -> bytes:
        out = bytearray()
        last_progress = time.monotonic()
        while len(out) < n:
            try:
                chunk = self._sock.recv(n - len(out))
            except socket.timeout as exc:
                if not out:
                    raise TimeoutError("tcp read timed out") from exc
                if (time.monotonic() - last_progress) >= _mid_frame_dead_s(self):
                    raise MidFrameTimeout(
                        "tcp read stalled mid-frame with no progress"
                    ) from exc
                continue
            if not chunk:
                break
            out += chunk
            last_progress = time.monotonic()
        return bytes(out)

    def write(self, data: bytes) -> int:
        self._sock.sendall(data)
        return len(data)

    def flush(self) -> None:  # noqa: D401
        return None

    def close(self) -> None:
        try:
            self._sock.close()
        except OSError:
            pass

    def cancel_pending_io(self, _thread_id: int | None) -> None:
        """Unblock a concurrent ``recv()`` on another thread before ``close()``.

        The thread id is accepted for call compatibility and ignored — TCP
        ``shutdown()`` unblocks *any* thread reading this socket, on every
        platform.
        """
        try:
            self._sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
