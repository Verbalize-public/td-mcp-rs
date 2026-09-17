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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tool", nargs="?", default="fleet")
    parser.add_argument("arguments", nargs="?", default="{}")
    parser.add_argument("--binary", default="tdmcp-daemon")
    parser.add_argument("--resource")
    parser.add_argument("--out", type=Path, help="Save the full MCP result including image content")
    args = parser.parse_args()
    if args.out and args.out.exists():
        parser.error("output already exists; inspect prior evidence before repeating a call")
    with StdioMcpClient(args.binary) as client:
        result = (client.request("resources/read", {"uri": args.resource}) if args.resource
                  else client.call_raw(args.tool, json.loads(args.arguments)))
    text = json.dumps(result, indent=2)
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        with args.out.open("x", encoding="utf-8") as output:
            output.write(text + "\n")
        print(args.out)
    else:
        print(text)


if __name__ == "__main__":
    main()
