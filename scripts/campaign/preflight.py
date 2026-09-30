#!/usr/bin/env python3
"""Decide whether a campaign round is allowed to run at all.

Every check here exists because a campaign that runs on an untrustworthy
stage produces confident nonsense. The gate suite is green while a
TouchDesigner is starved of CPU, and a `bridge heartbeat pong timeout` then
reads exactly like the supersede bug it is not: round 36 measured that
signature at load average 5-7 with three TouchDesigner instances and an
unrelated process at 90%. A refused run is a cheaper, more honest result
than a verdict.

Each check returns a Finding-shaped result, so the preflight report and the
scenario report have one vocabulary.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path

# The campaign's own ports. 9860/9861 belong to the user's installed daemon
# and are never touched: a campaign daemon that bound either of them would
# fail to start, or worse, displace a live install.
BRIDGE_PORT = 9987
MCP_PORT = 9988

# Above this the host is busy enough that a bridge heartbeat timeout is
# uninformative. Measured, not guessed: see module docstring.
LOAD_LIMIT = 8.0

# Ports that must be free before the round starts. Anything listening here
# means a previous round leaked, and a leaked daemon makes this round's
# measurements describe the previous one's debris.
REQUIRED_PORTS = (BRIDGE_PORT, MCP_PORT)


@dataclass
class Check:
    """One preflight verdict."""

    name: str
    passed: bool
    detail: str = ""
    # A failed check that only makes the round INCONCLUSIVE rather than wrong.
    fatal: bool = True

    def render(self) -> str:
        verdict = "PASS" if self.passed else ("INCONCLUSIVE" if not self.fatal else "FAIL")
        return f"[{verdict}] {self.name}: {self.detail}"


@dataclass
class Preflight:
    checks: list[Check] = field(default_factory=list)

    def add(self, name: str, passed: bool, detail: str = "", fatal: bool = True) -> None:
        self.checks.append(Check(name, passed, detail, fatal))

    @property
    def ok(self) -> bool:
        """True only when every fatal check passed."""
        return all(c.passed for c in self.checks if c.fatal)

    @property
    def inconclusive(self) -> bool:
        """A non-fatal failure still blocks the round, but reports as inconclusive."""
        return any(not c.passed for c in self.checks)

    def render(self) -> str:
        return "\n".join(c.render() for c in self.checks)

    def as_dict(self) -> dict:
        return {
            "ok": self.ok,
            "inconclusive": self.inconclusive,
            "checks": [
                {
                    "name": c.name,
                    "passed": c.passed,
                    "fatal": c.fatal,
                    "detail": c.detail,
                }
                for c in self.checks
            ],
        }


def load_average() -> float:
    """1-minute load average, or a large value when /proc cannot supply it."""
    try:
        return os.getloadavg()[0]
    except (OSError, AttributeError):  # pragma: no cover - platform guard
        return LOAD_LIMIT + 1.0


def listening_ports() -> set[int]:
    """Ports with a listener, read from /proc so no external tool is needed."""
    found: set[int] = set()
    for path in ("/proc/net/tcp", "/proc/net/tcp6"):
        try:
            lines = Path(path).read_text(encoding="utf-8", errors="replace").splitlines()[1:]
        except OSError:
            continue
        for line in lines:
            fields = line.split()
            if len(fields) < 4 or fields[3] != "0A":  # 0A = TCP_LISTEN
                continue
            try:
                found.add(int(fields[1].rsplit(":", 1)[1], 16))
            except (ValueError, IndexError):
                continue
    return found


def touchdesigner_pids() -> list[int]:
    """Every live TouchDesigner pid on the host.

    Ownership is decided by provenance, not by name: the campaign records the
    pids it spawned and asserts that the set of live pids afterwards is a
    subset of what it owns plus what the user already had. A campaign that
    finds an unexpected instance reports it rather than killing it.
    """
    pids: list[int] = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            cmdline = (entry / "cmdline").read_bytes().decode("utf-8", "replace")
        except OSError:
            continue
        if "TouchDesigner.exe" in cmdline:
            pids.append(int(entry.name))
    return sorted(pids)


def _git(repo: Path, *args: str) -> str:
    try:
        return subprocess.run(
            ["git", *args],
            cwd=repo,
            capture_output=True,
            text=True,
            timeout=30,
            check=True,
        ).stdout.strip()
    except (subprocess.SubprocessError, OSError):
        return ""


def installed_bridge_matches(install_dir: Path, repo_bridge: Path) -> tuple[bool, str]:
    """Is the bridge TD will import the same code the campaign tests?

    Round 36 ran a live session against a stale Wine-prefix copy and nearly
    reported a result about the wrong artifact. The daemon's `bridgePackageDir`
    normally wins, but that is a property to verify, not to assume.
    """
    if not install_dir.is_dir():
        return False, f"no install at {install_dir}"
    differing: list[str] = []
    checked = 0
    for source in sorted(repo_bridge.glob("*.py")):
        target = install_dir / source.name
        if not target.is_file():
            differing.append(f"{source.name} (missing)")
            continue
        checked += 1
        if source.read_bytes() != target.read_bytes():
            differing.append(source.name)
    if differing:
        return False, f"{len(differing)}/{checked} differ: {', '.join(differing[:4])}"
    return True, f"{checked} bridge files identical to the repo"


def run(
    repo: Path,
    *,
    install_dir: Path,
    baseline_td_pids: list[int] | None = None,
    require_clean_tree: bool = True,
    require_gate_evidence: Path | None = None,
) -> Preflight:
    """Collect every preflight check for a round."""
    pf = Preflight()

    head = _git(repo, "rev-parse", "--short", "HEAD")
    dirty = _git(repo, "status", "--porcelain")
    dirty_lines = [line for line in dirty.splitlines() if line]
    pf.add(
        "clean-tree",
        (not dirty_lines) if require_clean_tree else True,
        f"HEAD {head}, {len(dirty_lines)} uncommitted"
        + (f": {dirty_lines[0]}" if dirty_lines else ""),
    )

    if require_gate_evidence is not None:
        ok = require_gate_evidence.is_file()
        detail = "no gate evidence supplied"
        if ok:
            try:
                evidence = json.loads(require_gate_evidence.read_text(encoding="utf-8"))
                failed = [k for k, v in evidence.get("gates", {}).items() if v != "ok"]
                ok = not failed
                detail = "all gates ok" if ok else f"failed: {', '.join(failed)}"
            except (OSError, ValueError) as exc:
                ok, detail = False, f"unreadable: {exc}"
        pf.add("gate-evidence", ok, detail)

    load = load_average()
    pf.add(
        "host-load",
        load <= LOAD_LIMIT,
        f"load average {load:.2f} (limit {LOAD_LIMIT}); "
        "above the limit a bridge heartbeat timeout is uninformative, not a verdict",
        # Not fatal-as-wrong, but still blocks: a busy host cannot judge a
        # timeout, and reporting one as a defect would be a false finding.
        fatal=False,
    )

    busy = sorted(set(listening_ports()) & set(REQUIRED_PORTS))
    pf.add(
        "campaign-ports-free",
        not busy,
        f"{busy} still listening" if busy else f"{list(REQUIRED_PORTS)} free",
    )

    tds = touchdesigner_pids()
    if baseline_td_pids is None:
        pf.add(
            "td-baseline-recorded",
            True,
            f"{len(tds)} live TD instance(s) recorded as the user's: {tds}",
            # Informational: the user's own instances are not a fault. What
            # matters is that the campaign knows which ones it does not own.
            fatal=False,
        )
    else:
        appeared = sorted(set(tds) - set(baseline_td_pids))
        pf.add(
            "td-no-unknown-instances",
            not appeared,
            f"unexpected TD pids since baseline: {appeared}"
            if appeared
            else f"only the {len(tds)} baseline instance(s) live",
        )

    try:
        nproc = len(os.sched_getaffinity(0))
    except AttributeError:  # pragma: no cover - platform guard
        nproc = os.cpu_count() or 1
    pf.add("cpu-budget", nproc >= 2, f"{nproc} usable cpus", fatal=False)

    disk = shutil.disk_usage(repo)
    free_gb = disk.free / 1024**3
    pf.add("disk", free_gb >= 2.0, f"{free_gb:.1f} GiB free")

    matched, detail = installed_bridge_matches(
        install_dir / "bridge" / "tdmcp_bridge", repo / "bridge" / "tdmcp_bridge"
    )
    pf.add("installed-bridge-matches-repo", matched, detail)

    return pf


def main(argv: list[str]) -> int:
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=str(Path(__file__).resolve().parents[2]))
    parser.add_argument(
        "--install-dir", default=str(Path.home() / ".local/share/tdmcp-rs")
    )
    parser.add_argument("--baseline-td-pids", default="")
    parser.add_argument("--allow-dirty", action="store_true")
    parser.add_argument("--gate-evidence", default="")
    args = parser.parse_args(argv)

    baseline = (
        [int(p) for p in args.baseline_td_pids.split(",") if p.strip()]
        if args.baseline_td_pids
        else None
    )
    pf = run(
        Path(args.repo),
        install_dir=Path(args.install_dir),
        baseline_td_pids=baseline,
        require_clean_tree=not args.allow_dirty,
        require_gate_evidence=Path(args.gate_evidence) if args.gate_evidence else None,
    )
    print(pf.render())
    print(f"\nload={load_average():.2f}  td={touchdesigner_pids()}  ts={int(time.time())}")
    return 0 if pf.ok else 1


if __name__ == "__main__":
    import sys

    raise SystemExit(main(sys.argv[1:]))
