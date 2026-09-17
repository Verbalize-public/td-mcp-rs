#!/usr/bin/env python3
"""Sequential manual MCP stdio client. Owns only its proxy subprocess, not TD.

Calls are never retried after a timeout: reconcile live state before retrying.
"""
import argparse
import json
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time


class StdioMcpClient:
    def __init__(self, binary="tdmcp-daemon", timeout=90):
        self.timeout = timeout
        self.sequence = 0
        self.messages = queue.Queue()
        self.log = tempfile.TemporaryFile(mode="w+b")
        self.process = subprocess.Popen([str(binary), "mcp"], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=self.log)
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        try:
            self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                         "clientInfo": {"name": "tdmcp-stdio-probe", "version": "1"}})
            self.request("notifications/initialized", {}, notification=True)
        except BaseException:
            self.close()
            raise

    def _read(self):
        try:
            for line in self.process.stdout:
                if line.strip():
                    self.messages.put(json.loads(line))
        except Exception as error:
            self.messages.put(error)
        finally:
            self.messages.put(EOFError("stdio proxy closed"))

    def request(self, method, params, notification=False):
        self.sequence += 1
        ident = self.sequence
        message = {"jsonrpc": "2.0", "method": method, "params": params}
        if not notification:
            message["id"] = ident
        self.process.stdin.write((json.dumps(message) + "\n").encode())
        self.process.stdin.flush()
        if notification:
            return None
        deadline = time.monotonic() + self.timeout
        while True:
            try:
                reply = self.messages.get(timeout=max(0, deadline - time.monotonic()))
            except queue.Empty as error:
                raise TimeoutError("MCP wait expired; operation may still be running, do not replay") from error
            if isinstance(reply, Exception):
                raise reply
            if reply.get("id") != ident:
                continue
            if "error" in reply:
                raise RuntimeError(reply["error"])
            return reply["result"]

    def call_raw(self, name, arguments):
        return self.request("tools/call", {"name": name, "arguments": arguments})

    def call(self, name, arguments):
        raw = self.call_raw(name, arguments)
        if raw.get("isError"):
            raise RuntimeError(raw.get("structuredContent", raw))
        return raw.get("structuredContent", raw)

    def close(self):
        if self.process.stdin and not self.process.stdin.closed:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            try:
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        self.reader.join(timeout=1)
        self.process.stdout.close()
        self.log.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


def canonical_bytes(value):
    return len(json.dumps(value, sort_keys=True, separators=(",", ":"),
                          ensure_ascii=False).encode("utf-8"))


def measure(result):
    blocks = result.get("content") or []
    resources = list(result.get("contents") or [])
    resources.extend(b["resource"] for b in blocks if b.get("type") == "resource")
    images = [b["data"] for b in blocks if b.get("type") == "image"]
    images.extend(r["blob"] for r in resources
                  if "blob" in r and r.get("mimeType", "").startswith("image/"))
    texts = [b["text"] for b in blocks if b.get("type") == "text"]
    texts.extend(r["text"] for r in resources if "text" in r)
    return {
        "canonical_bytes": canonical_bytes(result),
        "structured_bytes": (canonical_bytes(result["structuredContent"])
                             if "structuredContent" in result else 0),
        "text_bytes": sum(len(text.encode("utf-8")) for text in texts),
        "image_count": len(images),
        "encoded_image_bytes": sum(len(image.encode("utf-8")) for image in images),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tool", nargs="?", default="fleet")
    parser.add_argument("arguments", nargs="?", default="{}")
    parser.add_argument("--binary", default="tdmcp-daemon")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--resource")
    mode.add_argument("--list-tools", action="store_true", help="Request tools/list for a catalog baseline")
    parser.add_argument("--measure", action="store_true",
                        help="Print byte metrics, not token counts: full MCP result and structuredContent "
                        "use sorted-key compact UTF-8 JSON; text and encoded images use UTF-8 payload bytes")
    parser.add_argument("--out", type=Path, help="Save the full MCP result including image content")
    args = parser.parse_args()
    if args.out and args.out.exists():
        parser.error("output already exists; inspect prior evidence before repeating a call")
    with StdioMcpClient(args.binary) as client:
        if args.list_tools:
            result = client.request("tools/list", {})
        else:
            result = (client.request("resources/read", {"uri": args.resource}) if args.resource
                      else client.call_raw(args.tool, json.loads(args.arguments)))
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        with args.out.open("x", encoding="utf-8") as output:
            output.write(json.dumps(result, indent=2) + "\n")
    if args.measure:
        print(json.dumps(measure(result), sort_keys=True))
    elif args.out:
        print(args.out)
    else:
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
