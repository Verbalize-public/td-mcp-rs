#!/usr/bin/env python3
"""Validate classified documentation examples without executing project code.

Full JSON Schema validation uses checked-in derived schema fixtures; Cargo's
schema golden test independently verifies those fixtures against current types.
The Rust example test uses the actual Serde input decoders as a second boundary.
"""
from __future__ import annotations

import ast
import hashlib
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
TEMPLATES = ROOT / "skills/templates"
REGISTRY = ROOT / "skills/EXAMPLES.json"


def blocks(text: str):
    """Yield outer fenced blocks, respecting longer fences around examples."""
    fence = None
    language = ""
    lines = []
    index = 0
    for line in text.splitlines():
        match = re.fullmatch(r"(`{3,})([^`]*)", line)
        if fence is None:
            if match:
                fence, language = match[1], match[2].strip()
                lines = []
        elif line.strip() == "`" * len(fence):
            yield index, language, "\n".join(lines)
            index += 1
            fence = None
        else:
            lines.append(line)
    if fence is not None:
        raise ValueError("unclosed Markdown fence")


def inventory():
    found = {}
    for path in sorted(TEMPLATES.rglob("*.jinja.md")):
        relative = path.relative_to(ROOT).as_posix()
        for index, language, body in blocks(path.read_text(encoding="utf-8")):
            found[f"{relative}:{index}"] = (language, body)
    return found


def require_local_refs(value):
    """Forbid remote schema resolution before handing a schema to the validator."""
    if isinstance(value, dict):
        ref = value.get("$ref")
        if ref is not None and (not isinstance(ref, str) or not ref.startswith("#")):
            raise ValueError("documentation schemas must use local references only")
        for child in value.values():
            require_local_refs(child)
    elif isinstance(value, list):
        for child in value:
            require_local_refs(child)


def validate(root: Path = ROOT):
    # Deliberately no network schema lookup: all references must resolve locally.
    from jsonschema import Draft202012Validator

    if root != ROOT:
        raise ValueError("alternate roots are not supported")
    data = json.loads(REGISTRY.read_text(encoding="utf-8"))
    found = inventory()
    registered = {row["id"]: row for row in data["blocks"]}
    if len(registered) != len(data["blocks"]) or registered.keys() != found.keys():
        raise ValueError("example inventory drift: classify every added/removed block")
    request_count = 0
    for key, (language, body) in found.items():
        row = registered[key]
        if row["sha256"] != hashlib.sha256(body.encode()).hexdigest():
            raise ValueError(f"example changed without classification review: {key}")
        if row["language"] != language or not row["context"] or not row["expected"]:
            raise ValueError(f"incomplete example context: {key}")
        kind = row["kind"]
        if kind in ("request", "request-fragment"):
            value = json.loads(body)
            if kind == "request-fragment":
                value = {"pid": 123, "steps": value if isinstance(value, list) else [value]}
            if value != row["arguments"]:
                raise ValueError(f"decoder fixture drift: {key}")
            schema = json.loads((ROOT / "crates/tdmcp-mcp/tests/fixtures/schemas" / (row["tool"] + ".json")).read_text())
            require_local_refs(schema)
            Draft202012Validator.check_schema(schema)
            Draft202012Validator(schema).validate(value)
            request_count += 1
        elif kind == "python-fragment":
            ast.parse(body)
        elif kind != "explanatory-fragment":
            raise ValueError(f"unknown example kind {kind}: {key}")
    for example in data["negativeRequests"]:
        schema = json.loads((ROOT / "crates/tdmcp-mcp/tests/fixtures/schemas" / (example["tool"] + ".json")).read_text())
        require_local_refs(schema)
        if not list(Draft202012Validator(schema).iter_errors(example["arguments"])):
            raise ValueError(f"intentional negative is now accepted: {example['id']}")
    for example in data["nativeScripts"]:
        source = (ROOT / example["path"]).read_text()
        ast.parse(source)
        if hashlib.sha256(source.encode()).hexdigest() != example["sha256"]:
            raise ValueError(f"native script changed; reassess recorded scope: {example['path']}")
    return {"blocks": len(found), "requests": request_count,
            "negativeRequests": len(data["negativeRequests"]),
            "nativeScriptsSyntaxOnly": len(data["nativeScripts"])}


if __name__ == "__main__":
    try:
        print(json.dumps(validate(), sort_keys=True))
    except ImportError as exc:
        print(f"Missing test dependency: {exc}; install scripts/requirements-test.txt", file=sys.stderr)
        sys.exit(1)
