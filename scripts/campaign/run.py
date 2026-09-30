#!/usr/bin/env python3
"""Run a campaign round and write a private run record.

    .venv/bin/python scripts/campaign/run.py --scenario lifecycle
    .venv/bin/python scripts/campaign/run.py --all --fault-inject

The record lands in `retroanalysis/campaign/<run-id>/` -- a gitignored path,
so transcripts of what the live tools actually answered stay on this machine
instead of becoming commit history. What DOES get committed is the verdict
summary, because a campaign whose results are all private cannot be reviewed.

Exit codes: 0 verified, 1 a real finding, 2 inconclusive (host, transport, or
an unreachable roster), 3 the round refused to start.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))
if str(REPO / "scripts") not in sys.path:
    sys.path.insert(0, str(REPO / "scripts"))

import coverage as coverage_map  # noqa: E402
import preflight as preflight_mod  # noqa: E402
from harness import BRIDGE_PORT, MCP_PORT, Session  # noqa: E402
from scenario import Refusal, Scenario, ScenarioResult, load_scenarios, run_scenario  # noqa: E402

RECORD_ROOT = REPO / "retroanalysis" / "campaign"
SCENARIO_DIR = HERE / "scenarios"


def _new_run_id() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")


class FaultInjection(Exception):
    """Raised by the injected fault, so a fault run cannot pass by accident."""


def make_faulty_call(real_call):
    """Wrap a real call so exactly one assertion in the corpus fails.

    The pilot's second exit criterion: a runner that cannot fail a known
    fault is not a runner. The fault is deliberately small and named -- a
    genuine `ok:true` answer is rewritten to `ok:false` -- so the failure
    that appears is attributable to the injection and to nothing else.
    """

    def call(tool: str, args: dict, timeout_s: int = 120) -> dict:
        payload = real_call(tool, args, timeout_s)
        if tool == "fleet" and not args.get("pids"):
            mutated = dict(payload)
            mutated["items"] = [{"ok": False, "summary": "INJECTED FAULT: fleet was made to fail"}]
            mutated["ok"] = False
            return mutated
        return payload

    return call


def resolve_scenarios(selection: list[str]) -> list[Scenario]:
    scenarios = load_scenarios(SCENARIO_DIR)
    if not selection or "all" in selection:
        return scenarios
    wanted = set(selection)
    found = [s for s in scenarios if s.id in wanted]
    missing = wanted - {s.id for s in found}
    if missing:
        raise SystemExit(f"unknown scenario id(s): {sorted(missing)}; have {[s.id for s in scenarios]}")
    return found


def build_parser() -> argparse.ArgumentParser:
    """The real parser, so a docstring example can be checked against it.

    Extracted rather than built inline so the usage text is verifiable: an
    example that does not parse is a false promise about how to run the
    thing, and the only way to know is to parse the real thing.
    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", action="append", default=[], help="scenario id (repeatable) or 'all'")
    parser.add_argument(
        "--all",
        action="store_true",
        help="run every scenario (the same as --scenario all, spelled as a flag)",
    )
    parser.add_argument("--fault-inject", action="store_true", help="prove the runner can fail")
    parser.add_argument("--no-preflight", action="store_true")
    parser.add_argument("--run-id", default="")
    parser.add_argument("--keep-td", action="store_true", help="debug only: do not stop the spawned TD")
    return parser


def fault_verdict(results: list[ScenarioResult]) -> tuple[str, str]:
    """Did a fault-injection run actually DETECT anything?

    Returns `(verdict, line_to_print)`. Extracted so the decision can be
    tested rather than trusted, because the tempting version -- `not
    all_verified` -- is vacuous and was measured to be: a scenario whose SETUP
    failed is `inconclusive`, never `passed`, so the shortcut reports "the
    injected fault was caught" for a run in which the fault was never on
    screen. "Did not pass" and "detected a defect" are different claims, and
    only the second one is evidence that the harness can fail.
    """
    defects = [r.id for r in results if r.verdict == "defect" and not r.passed]
    if defects:
        return (
            "verified (the injected fault was caught)",
            f"FAULT INJECTION: caught by {defects}",
        )
    ran = [r.id for r in results if r.results]
    why = (
        "no scenario reported a defect verdict"
        if ran
        else "no scenario ran at all -- the injected fault was never exercised"
    )
    return "HARNESS FAILS TO DETECT", f"FAULT INJECTION: NOT CAUGHT ({why})"


def main(argv: list[str]) -> int:
    args = build_parser().parse_args(argv)

    if args.all:
        # Both spellings work because the usage text in this module's own
        # docstring shows `--all`; an example that fails to parse is a false
        # promise about how to run the thing.
        args.scenario = ["all"]

    run_id = args.run_id or _new_run_id()
    run_dir = RECORD_ROOT / run_id

    # Resolve the selection BEFORE anything expensive. A typo'd scenario id
    # should cost a parse, not a full preflight sweep and a run directory:
    # paying for the check only after the work is what makes a mistyped id
    # look like a campaign that started and then failed.
    scenarios = resolve_scenarios(args.scenario)
    if not scenarios:
        print("no scenarios selected")
        return 2

    run_dir.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    print(f"campaign run {run_id}\nrecord: {run_dir}")

    # --- preflight: decide whether the stage is trustworthy ---------------
    baseline_td = preflight_mod.touchdesigner_pids()
    pf = preflight_mod.run(
        REPO,
        install_dir=Path.home() / ".local/share/tdmcp-rs",
        baseline_td_pids=None,
        require_clean_tree=False,  # the harness itself is uncommitted during a pilot
    )
    (run_dir / "preflight.json").write_text(json.dumps(pf.as_dict(), indent=2), encoding="utf-8")
    print("\n" + pf.render())
    if not args.no_preflight and not pf.ok:
        print(f"\nREFUSED: {pf.render()}")
        return 3
    if pf.inconclusive:
        print("NOTE: running with a non-fatal preflight failure; timeouts will be INCONCLUSIVE, not defects")

    data_dir = run_dir / "daemon-data"
    session = Session(data_dir=data_dir, run_dir=run_dir)
    results = []
    teardown_report: dict = {}
    fleet_before: object = None
    try:
        print(f"\nstarting task-owned daemon: bridge={BRIDGE_PORT} mcp={MCP_PORT} data={data_dir}")
        session.start_daemon()
        print(f"daemon up: {MCP_PORT}")

        # The roster is read from the task-owned daemon, not the user's.
        roster = coverage_map.live_tool_names(lambda t, a: session.call(t, a))
        if roster is None:
            print("roster unknown from the task daemon; recording coverage as unknown")
        else:
            report = coverage_map.build(roster, scenarios)
            (run_dir / "coverage.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
            print(coverage_map.render(report))

        real_call = session.call
        call = make_faulty_call(real_call) if args.fault_inject else real_call

        for scenario in scenarios:
            print(f"\n--- {scenario.id}: {scenario.title}")
            if not scenario.needs_td:
                result = run_scenario(scenario, call, ctx={"pid": None}, out_dir=run_dir / scenario.id)
            else:
                print("    spawning a task-owned TouchDesigner (no project file)...")
                try:
                    pid = session.spawn_td()
                    print(f"    td pid {pid}, bridge connected")
                    fleet_before = session.call("fleet", {"include": ["process", "bridge"]}, 30)
                    ctx = {"pid": pid}
                    result = run_scenario(
                        scenario,
                        call,
                        ctx=ctx,
                        out_dir=run_dir / scenario.id,
                    )
                except Exception as exc:
                    # A spawn or handshake failure is a finding about the
                    # lifecycle, not a reason to skip the scenario silently.
                    result = ScenarioResult(
                        id=scenario.id,
                        title=scenario.title,
                        passed=False,
                        verdict="inconclusive",
                        findings=[f"scenario setup failed: {type(exc).__name__}: {exc}"],
                    )
            results.append(result)
            for step in result.results:
                mark = "ok  " if step.passed else "FAIL"
                print(f"    [{mark}] {step.name}: {step.observed[:220]}")
            if result.findings:
                for finding in result.findings:
                    print(f"    !! {finding}")
            if not scenario.needs_td:
                continue
            if args.keep_td:
                print(f"    --keep-td: leaving pid {session.td_pid}")
            else:
                session.stop_td()

    except Exception as exc:
        print(f"ROUND ABORTED: {type(exc).__name__}: {exc}")
        results.append(None)
    finally:
        teardown_report = session.teardown()
        print(f"\nteardown: {json.dumps(teardown_report)}")
        remaining = preflight_mod.touchdesigner_pids()
        leaked = sorted(set(remaining) - set(baseline_td))
        print(f"TD after teardown: {remaining}  (baseline {baseline_td}, leaked {leaked})")
        teardown_report["td_after"] = remaining
        teardown_report["td_leaked"] = leaked
        if session.log_lines:
            (run_dir / "daemon.log").write_text("\n".join(session.log_lines), encoding="utf-8")
        if session.log_noise:
            # The child processes (TouchDesigner, Wine) share the daemon's pipe.
            # Recorded separately and unfiltered: a `GLXBadDrawable` in here is
            # evidence about the child's display, and losing it is how a
            # display problem reads as a mysterious silence.
            (run_dir / "child-noise.log").write_text(
                "\n".join(session.log_noise), encoding="utf-8"
            )

    # --- verdict ----------------------------------------------------------
    real_results = [r for r in results if r is not None]
    any_defect = any(not r.passed and r.verdict == "defect" for r in real_results)
    any_inconclusive = any(r.verdict == "inconclusive" for r in real_results) or len(real_results) != len(results)
    all_verified = bool(real_results) and all(r.passed for r in real_results)

    if args.fault_inject:
        verdict, line = fault_verdict(real_results)
        print(f"\n{line}")
    elif any_defect:
        verdict = "defect"
    elif any_inconclusive:
        verdict = "inconclusive"
    elif all_verified:
        verdict = "verified"
    else:
        verdict = "inconclusive"

    # A leak is never folded into a scenario verdict. A campaign that reports
    # "verified" while leaving a TouchDesigner running is telling the next
    # round that the host was clean when it was not, and the next round's
    # measurements will describe this round's debris. Named here so it cannot
    # be read as part of the scenario outcome.
    leaked_td = teardown_report.get("td_leaked") or []
    if leaked_td and verdict == "verified":
        verdict = "inconclusive"
        print(f"\n!! LEAKED TouchDesigner {leaked_td} -- the run cannot be read as clean")

    summary = {
        "run_id": run_id,
        "verdict": verdict,
        "fault_inject": args.fault_inject,
        "wall_s": round(time.monotonic() - started, 1),
        "preflight": pf.as_dict(),
        "baseline_td": baseline_td,
        "teardown": teardown_report,
        "scenarios": [r.as_dict() for r in real_results],
    }
    (run_dir / "summary.json").write_text(json.dumps(summary, indent=2, default=str), encoding="utf-8")
    print(f"\nverdict: {verdict}   wall: {summary['wall_s']}s   record: {run_dir}")

    if args.fault_inject:
        return 0 if fault_verdict(real_results)[0].startswith("verified") else 1
    if any_defect:
        return 1
    if any_inconclusive or not all_verified or leaked_td:
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
