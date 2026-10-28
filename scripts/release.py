#!/usr/bin/env python3
"""Pinned, resumable release qualification. Only qualified manifests admit assets."""
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib

TARGETS = (
    "x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc",
    "aarch64-apple-darwin", "x86_64-apple-darwin",
)


def run(*args, check=True):
    return subprocess.run(args, check=check, capture_output=True, text=True)


def gh(*args):
    return run("gh", *args).stdout


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def prepare():
    sha = run("git", "rev-parse", "HEAD").stdout.strip()
    requested = os.environ.get("REQUESTED_COMMIT", "")
    if os.environ["GITHUB_EVENT_NAME"] == "workflow_dispatch" and requested != sha:
        raise ValueError("commit must be the full SHA of the selected workflow ref")
    version = tomllib.loads(Path("Cargo.toml").read_text())["workspace"]["package"]["version"]
    tag = f"v{version}"
    if os.environ["GITHUB_EVENT_NAME"] == "push" and os.environ["GITHUB_REF_NAME"] != tag:
        raise ValueError("tag does not match workspace version; tags must never be moved")
    existing = run("git", "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}", check=False)
    if existing.returncode == 0 and existing.stdout.strip() != sha:
        raise ValueError(f"{tag} already identifies another commit")
    mode = os.environ["RELEASE_MODE"]
    if mode not in ("build-only", "draft", "publish"):
        raise ValueError("unknown release mode")
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"version={version}\nsha={sha}\nmode={mode}\n")


def expected_files(version, target):
    stem = f"tdmcp-rs-{version}-{target}"
    if "windows" in target:
        return {f"{stem}.zip", f"tdmcp-rs-{version}-x64-setup.exe"}
    files = {f"{stem}.tar.gz"}
    if "apple" in target:
        files.add(f"{stem}.dmg")
    return files


def qualify():
    target = os.environ["RELEASE_TARGET"]
    if target not in TARGETS:
        raise ValueError("unknown target")
    version = tomllib.loads(Path("Cargo.toml").read_text())["workspace"]["package"]["version"]
    files = expected_files(version, target)
    manifest = {"sha": os.environ["RELEASE_SHA"], "target": target,
                "version": version, "files": {name: digest(Path("dist") / name) for name in sorted(files)}}
    Path(f"dist/qualified-{target}.json").write_text(json.dumps(manifest, indent=2) + "\n")


def validated_assets(directory, sha, version):
    assets = {}
    qualified = set()
    for path in sorted(directory.glob("qualified-*.json")):
        manifest = json.loads(path.read_text())
        target = manifest["target"]
        if (target not in TARGETS or manifest["sha"] != sha or manifest["version"] != version
                or path.name != f"qualified-{target}.json"
                or set(manifest["files"]) != expected_files(version, target)):
            raise ValueError(f"invalid qualification: {path.name}")
        for name, checksum in manifest["files"].items():
            candidate = directory / name
            if candidate.is_symlink() or digest(candidate) != checksum:
                raise ValueError(f"checksum mismatch: {name}")
            assets[name] = checksum
        assets[path.name] = digest(path)
        qualified.add(target)
    return assets, qualified


def report_failures(qualified, sha):
    repo, run_id = os.environ["GH_REPO"], os.environ["GITHUB_RUN_ID"]
    workflow = f"https://github.com/{repo}/actions/runs/{run_id}"
    pages = json.loads(gh("api", "--paginate", "--slurp", f"repos/{repo}/actions/runs/{run_id}/jobs?filter=latest&per_page=100"))
    jobs = [job for page in pages for job in page["jobs"]]
    issues = json.loads(gh("issue", "list", "--state", "open", "--limit", "1000", "--json", "number,title"))
    for target in TARGETS:
        if target in qualified:
            continue
        job = next((job for job in jobs if target in job["name"]), None)
        steps = [step["name"] for step in (job or {}).get("steps", []) if step["conclusion"] == "failure"]
        excerpt = "No qualified artifact was produced."
        if job:
            log = run("gh", "api", f"repos/{repo}/actions/jobs/{job['id']}/logs", check=False)
            if "allow-escape-sequences" in log.stdout + log.stderr:
                log = run("gh", "api", f"repos/{repo}/actions/jobs/{job['id']}/logs", "--allow-escape-sequences", check=False)
            excerpt = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", log.stdout or log.stderr or excerpt)[-6000:]
        title = f"Release platform failure: {target}"
        body = f"Target: `{target}`\nCommit: `{sha}`\nFailed step: {', '.join(steps) or 'qualification/upload'}\nWorkflow: {workflow}\n\n```text\n{excerpt}\n```\n"
        with tempfile.NamedTemporaryFile("w", delete=False) as output:
            output.write(body)
            filename = output.name
        try:
            issue = next((item for item in issues if item["title"] == title), None)
            if issue:
                gh("issue", "edit", str(issue["number"]), "--body-file", filename)
            else:
                gh("issue", "create", "--title", title, "--body-file", filename)
        finally:
            Path(filename).unlink()


def publish():
    sha, version, mode = (os.environ[key] for key in ("RELEASE_SHA", "RELEASE_VERSION", "RELEASE_MODE"))
    tag = f"v{version}"
    directory = Path("dist")
    directory.mkdir(exist_ok=True)
    assets, qualified = validated_assets(directory, sha, version)
    report_failures(qualified, sha)
    if not qualified:
        raise ValueError("no platform qualified; nothing will be published")
    if mode == "build-only":
        print(f"Build-only: qualified {sorted(qualified)}")
        return
    repo = os.environ["GH_REPO"]
    # Recheck on the server immediately before creating the release. Never force a tag.
    remote = run("gh", "api", f"repos/{repo}/commits/{tag}", check=False)
    if remote.returncode == 0:
        if json.loads(remote.stdout)["sha"] != sha:
            raise ValueError("existing release tag identifies another commit")
    else:
        # A permission/network failure is not proof of absence: creation will fail closed.
        gh("api", "--method", "POST", f"repos/{repo}/git/refs", "-f", f"ref=refs/tags/{tag}", "-f", f"sha={sha}")
    existing = run("gh", "release", "view", tag, "--json", "isDraft,assets", check=False)
    if existing.returncode:
        gh("release", "create", tag, "--verify-tag", "--draft", "--title", tag, "--notes", f"Qualifying release commit {sha}")
        release = {"isDraft": True, "assets": []}
    else:
        release = json.loads(existing.stdout)
        if mode == "draft" and not release["isDraft"]:
            raise ValueError("draft mode cannot demote an already published release")
    existing_names = {asset["name"] for asset in release["assets"]}
    # Download and validate previous qualification records before resuming. Old
    # successful platforms remain admitted even when only failed jobs were rerun.
    with tempfile.TemporaryDirectory() as previous:
        previous = Path(previous)
        if existing_names:
            gh("release", "download", tag, "--dir", str(previous))
            old_assets, old_qualified = validated_assets(previous, sha, version)
            for name, checksum in assets.items():
                if name in existing_names and digest(previous / name) != checksum:
                    raise ValueError(f"refusing to replace existing asset: {name}")
            assets = old_assets | assets
            qualified |= old_qualified
        for name in sorted(assets):
            if name not in existing_names:
                gh("release", "upload", tag, str(directory / name))
    checksums = "".join(f"{checksum}  {name}\n" for name, checksum in sorted(assets.items()))
    (directory / "SHA256SUMS.txt").write_text(checksums)
    gh("release", "upload", tag, str(directory / "SHA256SUMS.txt"), "--clobber")
    missing = sorted(set(TARGETS) - qualified)
    notes = (f"Missing platforms: {', '.join(missing) if missing else 'none'}.\n\n"
             f"Commit: `{sha}`\n\nQualified platforms: {', '.join(sorted(qualified))}.\n\n"
             "Artifacts are unsigned (macOS may carry an ad-hoc signature); no publisher signing or notarization is claimed. "
             "Each platform passed dependency checks, native tests and an extracted-package MCP smoke test. "
             "Archives include third-party license records and required source inventories.\n\n"
             "Defaults: 180 daily logs, 180 days, 500 crash reports, 200 recent records per crash. "
             "Existing configured limits are preserved. Zero disables a cleanup limit.\n\n"
             f"[Workflow](https://github.com/{repo}/actions/runs/{os.environ['GITHUB_RUN_ID']})\n")
    (directory / "release-notes.md").write_text(notes)
    gh("release", "edit", tag, "--notes-file", str(directory / "release-notes.md"), "--draft=" + str(mode == "draft").lower())


if __name__ == "__main__":
    {"prepare": prepare, "qualify": qualify, "publish": publish}[sys.argv[1]]()
