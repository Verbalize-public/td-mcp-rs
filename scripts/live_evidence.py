"""Small evidence recorder for explicit, sequential, manually authorized MCP probes."""
import base64
import json
from pathlib import Path


class Evidence:
    def __init__(self, client, directory, pid):
        if not isinstance(pid, int) or isinstance(pid, bool) or pid <= 0:
            raise ValueError("explicit positive PID required")
        self.client = client
        self.directory = Path(directory)
        self.directory.mkdir(parents=True, exist_ok=True)
        self.pid = pid

    def call(self, tag, tool, arguments, *, expected_error=False, targeted=True):
        if not tag or any(c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-" for c in tag):
            raise ValueError("invalid evidence tag")
        args = dict(arguments)
        if targeted:
            if "pid" in args and args["pid"] != self.pid:
                raise ValueError("target changed")
            args["pid"] = self.pid
        request = self.directory / (tag + ".request.json")
        response = self.directory / (tag + ".response.json")
        if response.exists():
            raise FileExistsError(response)
        with request.open("x", encoding="utf-8") as stream:
            json.dump({"tool":tool,"arguments":args}, stream, indent=2)
        raw = self.client.call_raw(tool, args)
        with response.open("x", encoding="utf-8") as stream:
            json.dump(raw, stream, indent=2)
        for index, block in enumerate(raw.get("content", [])):
            if block.get("type") == "image":
                suffix = {"image/png":"png", "image/jpeg":"jpg"}.get(block.get("mimeType"))
                if suffix is None:
                    raise ValueError("unsupported evidence image format")
                path = self.directory / (tag + "_" + str(index) + "." + suffix)
                with path.open("xb") as stream:
                    stream.write(base64.b64decode(block["data"], validate=True))
        value = raw.get("structuredContent", raw)
        failed = bool(raw.get("isError")) or value.get("ok") is False
        if failed != expected_error:
            raise RuntimeError({"tag":tag,"unexpected_failure":failed,"result":value})
        return value
