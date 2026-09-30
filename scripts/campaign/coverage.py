#!/usr/bin/env python3
"""The coverage map: every tool, every scenario, no false completeness.

This exists because "we tested the product" is only a claim if something
knows what "the product" is. The tool roster is read from the live daemon
(`describe_tools`), not from a checked-in list, so a newly shipped tool shows
up as UNMAPPED instead of quietly inheriting the coverage of its neighbour.

`covered()` is deliberately strict. A scenario that merely NAMES a tool does
not cover it: the `tools` list has to be a claim someone checked, and
`--check` is the mode CI runs, which fails on any unmapped tool.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
if str(REPO / "scripts") not in sys.path:
    sys.path.insert(0, str(REPO / "scripts"))

from scenario import Scenario, load_scenarios  # noqa: E402

SCENARIO_DIR = Path(__file__).resolve().parent / "scenarios"


def live_tool_names(call) -> list[str] | None:
    """Ask the daemon what it serves, or None when it cannot be reached.

    `call` is a one-argument-bound callable (tool, args) -- the harness
    wraps the three-argument form. None is not an empty list: a daemon that
    cannot be asked means the coverage map is unknown, and reporting zero
    unmapped tools then would be the most dangerous possible lie this module
    could tell.
    """
    try:
        payload = call("describe_tools", {})
    except Exception:
        return None
    tools = payload.get("tools") if isinstance(payload, dict) else None
    if isinstance(tools, list):
        return [t.get("name") for t in tools if isinstance(t, dict) and t.get("name")]
    items = payload.get("items") if isinstance(payload, dict) else None
    if isinstance(items, list) and items:
        first = items[0]
        result = first.get("result", first) if isinstance(first, dict) else {}
        inner = result.get("tools") if isinstance(result, dict) else None
        if isinstance(inner, list):
            return [t.get("name") for t in inner if isinstance(t, dict) and t.get("name")]
    return None


def build(roster: list[str], scenarios: list[Scenario]) -> dict:
    mapped: dict[str, list[str]] = {name: [] for name in roster}
    for scenario in scenarios:
        for tool in scenario.tools:
            mapped.setdefault(tool, []).append(scenario.id)
    unmapped = sorted(name for name, ids in mapped.items() if not ids)
    scenario_ids = [s.id for s in scenarios]
    return {
        "roster": sorted(roster),
        "mapped": {k: sorted(v) for k, v in sorted(mapped.items()) if v},
        "unmapped": unmapped,
        "unknown_tools": sorted(set(mapped) - set(roster)),
        "scenarios": scenario_ids,
        "complete": not unmapped,
    }


def render(report: dict) -> str:
    lines = [f"tools: {len(report['roster'])}   scenarios: {len(report['scenarios'])}"]
    for name in report["roster"]:
        ids = report["mapped"].get(name)
        if ids:
            lines.append(f"  [ok]      {name:<26} {', '.join(ids)}")
        else:
            lines.append(f"  [UNMAPPED] {name}")
    if report["unknown_tools"]:
        lines.append(f"  scenarios name tools the daemon does not serve: {report['unknown_tools']}")
    lines.append("complete" if report["complete"] else f"INCOMPLETE: {len(report['unmapped'])} unmapped")
    return "\n".join(lines)


def main(argv: list[str]) -> int:
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail unless every tool is mapped")
    parser.add_argument("--roster", default="", help="JSON file with a tool roster (offline use)")
    parser.add_argument("--url", default="", help="daemon URL for a live roster (default TDMCP_DAEMON_URL)")
    args = parser.parse_args(argv)

    scenarios = load_scenarios(SCENARIO_DIR)
    roster: list[str] | None = None
    if args.roster:
        roster = json.loads(Path(args.roster).read_text(encoding="utf-8"))
        if isinstance(roster, dict):
            roster = roster.get("tools", [])
    else:
        from harness import McpClient  # re-exported so this works from any cwd

        client = McpClient(base=args.url or None)
        try:
            roster = live_tool_names(lambda tool, args: client.call(tool, args))
        finally:
            client.close()

    if roster is None:
        # Exit 2 is not 0 on purpose: an unreachable daemon means the roster is
        # UNKNOWN, and a caller that ignores the return code would otherwise
        # read this as "everything is mapped".
        print("coverage: UNKNOWN roster (daemon unreachable) -- not claiming completeness")
        return 2

    report = build(roster, scenarios)
    print(render(report))
    if args.check and not report["complete"]:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
