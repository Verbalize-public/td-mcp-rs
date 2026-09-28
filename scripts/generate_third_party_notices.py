#!/usr/bin/env python3
"""Generate deterministic per-target third-party notices from the locked release graph."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parent.parent
LOCK_PATH = ROOT / "Cargo.lock"
OUTPUT_ROOT = ROOT / "third-party" / "generated"
CANONICAL_ROOT = ROOT / "third-party" / "canonical"
TARGETS = (
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
)
NOTICE_PREFIXES = (
    "license",
    "licence",
    "copying",
    "copyright",
    "notice",
    "unlicense",
    "authors",
    "contributors",
    "patents",
)
# The font crate publishes its per-font license texts under names which do not
# all contain "license". Keep this exact list pinned to the locked crate.
EXTRA_NOTICE_FILES = {
    ("epaint_default_fonts", "0.35.0"): (
        "fonts/Hack-Regular.txt",
        "fonts/OFL.txt",
        "fonts/UFL.txt",
        "fonts/emoji-icon-font-mit-license.txt",
    ),
    # The sys crate's MIT expression covers its Rust wrapper. When pkg-config
    # cannot find FreeType, its build script compiles the bundled C library,
    # whose dual-license texts use historical names under docs/.
    ("freetype-sys", "0.20.1"): (
        "freetype2/docs/FTL.TXT",
        "freetype2/docs/GPLv2.TXT",
    ),
}
COMPONENT_LICENSE_NOTES = {
    ("freetype-sys", "0.20.1"): (
        "The crate's MIT expression covers its Rust wrapper; the bundled FreeType "
        "fallback includes the preserved FTL/GPLv2 choice. td-mcp-rs relies on FTL terms."
    ),
    ("wayland-protocols-plasma", "0.3.12"): (
        "The generated API crate declares MIT and vendors Plasma protocol XML carrying "
        "the additional terms preserved from plasma-wayland-protocols/LICENSES/."
    ),
}
SOURCE_ARCHIVE_REASONS = {
    ("option-ext", "0.2.0"): "MPL-2.0 covered software",
    ("wayland-protocols-plasma", "0.3.12"): (
        "vendored Plasma protocol XML includes LGPL-2.1-or-later covered source"
    ),
}
# Some crates inherit a workspace license but do not include that workspace's
# license files in the .crate archive. Pinned SPDX texts close that packaging
# gap. The dependency's authors and declared expression remain in the notice.
CANONICAL_FALLBACKS = {
    "MIT": "MIT.txt",
    "Apache-2.0": "Apache-2.0.txt",
    "CC0-1.0": "CC0-1.0.txt",
    "BSL-1.0": "BSL-1.0.txt",
}


def sha256_bytes(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load_lock() -> tuple[str, dict[tuple[str, str, str], dict]]:
    content = LOCK_PATH.read_bytes()
    parsed = tomllib.loads(content.decode("utf-8"))
    packages = {}
    for package in parsed["package"]:
        source = package.get("source")
        if source:
            packages[(package["name"], package["version"], source)] = package
    return sha256_bytes(content), packages


def cargo_metadata(target: str) -> dict:
    command = [
        "cargo",
        "metadata",
        "--locked",
        "--format-version",
        "1",
        "--filter-platform",
        target,
    ]
    result = subprocess.run(
        command,
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )
    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip()
        raise RuntimeError(f"{' '.join(command)} failed: {detail}")
    return json.loads(result.stdout)


def cargo_tree_identities(target: str) -> set[tuple[str, str]]:
    # Normal edges are the dependencies shipped in the executable. Build-only
    # tools are checked by cargo-deny but are not redistributed in the package.
    command = [
        "cargo",
        "tree",
        "--locked",
        "-p",
        "tdmcp-daemon",
        "--features",
        "gui",
        "--target",
        target,
        "--edges",
        "normal",
        "--prefix",
        "none",
        "--format",
        "{p}",
    ]
    result = subprocess.run(
        command,
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=180,
        check=False,
    )
    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip()
        raise RuntimeError(f"{' '.join(command)} failed: {detail}")
    identities = set()
    for line in result.stdout.splitlines():
        fields = line.split()
        if len(fields) >= 2 and fields[1].startswith("v"):
            identities.add((fields[0], fields[1][1:]))
    if not identities:
        raise RuntimeError(f"cargo tree returned no packages for {target}")
    return identities


def select_release_packages(metadata: dict, identities: set[tuple[str, str]]) -> list[dict]:
    selected = []
    for name, version in identities:
        all_matches = [
            package
            for package in metadata["packages"]
            if package["name"] == name and package["version"] == version
        ]
        if not all_matches:
            raise RuntimeError(f"cargo-tree package absent from metadata: {name} {version}")
        matches = [package for package in all_matches if package["source"]]
        if len(matches) > 1:
            sources = ", ".join(package["source"] for package in matches)
            raise RuntimeError(f"ambiguous sources for cargo-tree package {name} {version}: {sources}")
        if matches:
            selected.append(matches[0])
    return sorted(selected, key=lambda package: (package["name"], package["version"], package["source"]))


def declared_ids(expression: str) -> set[str]:
    return {identifier for identifier in CANONICAL_FALLBACKS if identifier in expression}


def notice_files(package: dict, crate_files: dict[str, bytes]) -> list[tuple[str, bytes, str]]:
    package_root = Path(package["manifest_path"]).parent
    selected: set[str] = set()
    configured = package.get("license_file")
    if configured:
        configured_path = Path(configured)
        if configured_path.is_absolute():
            try:
                configured_path = configured_path.relative_to(package_root)
            except ValueError as error:
                raise RuntimeError(
                    f"{package['name']} {package['version']} license-file escapes package root"
                ) from error
        selected.add(configured_path.as_posix())

    for relative in crate_files:
        parts = relative.split("/")
        in_license_directory = any(part.lower() in {"license", "licenses", "legal"} for part in parts[:-1])
        if parts[-1].lower().startswith(NOTICE_PREFIXES) or in_license_directory:
            selected.add(relative)

    key = (package["name"], package["version"])
    selected.update(EXTRA_NOTICE_FILES.get(key, ()))

    files: list[tuple[str, bytes, str]] = []
    for relative in sorted(selected):
        content = crate_files.get(relative)
        if content is None:
            raise RuntimeError(f"missing declared notice for {package['name']} {package['version']}: {relative}")
        files.append((relative, content, "package"))

    expression = package.get("license") or ""
    fallbacks = declared_ids(expression) if not files else set()
    # epaint_default_fonts has all font terms in unusual files but its crate
    # code's inherited MIT/Apache license files are absent from the .crate.
    if key == ("epaint_default_fonts", "0.35.0"):
        fallbacks.update({"MIT", "Apache-2.0"})
    for identifier in sorted(fallbacks):
        canonical = CANONICAL_ROOT / CANONICAL_FALLBACKS[identifier]
        if not canonical.is_file():
            raise RuntimeError(f"missing canonical SPDX text: {canonical.relative_to(ROOT)}")
        files.append((f"SPDX/{identifier}.txt", canonical.read_bytes(), "SPDX-3.27.0"))

    if not files:
        raise RuntimeError(
            f"no license/notice text found for {package['name']} {package['version']} "
            f"({expression or 'NOASSERTION'})"
        )
    return files


def registry_download_url(package: dict) -> str:
    return f"https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download"


def cached_crate(package: dict) -> Path:
    manifest = Path(package["manifest_path"])
    registry_key = manifest.parent.parent.name
    archive = manifest.parents[2].parent / "cache" / registry_key / f"{package['name']}-{package['version']}.crate"
    if not archive.is_file():
        raise RuntimeError(
            f"source archive unavailable for {package['name']} {package['version']}; "
            "run cargo fetch --locked and regenerate"
        )
    return archive


CRATE_CONTENT_CACHE: dict[tuple[str, str, str], tuple[Path, dict[str, bytes]]] = {}


def verified_crate_contents(package: dict, checksum: str) -> tuple[Path, dict[str, bytes]]:
    key = (package["name"], package["version"], package["source"])
    cached = CRATE_CONTENT_CACHE.get(key)
    if cached is not None:
        return cached

    archive = cached_crate(package)
    archive_hash = sha256_file(archive)
    if archive_hash != checksum:
        raise RuntimeError(
            f"cached source checksum mismatch for {package['name']} {package['version']}: "
            f"{archive_hash} != {checksum}"
        )
    prefix = f"{package['name']}-{package['version']}/"
    files = {}
    with tarfile.open(archive, "r:gz") as crate:
        for member in crate.getmembers():
            if not member.isfile() or not member.name.startswith(prefix):
                continue
            relative = member.name[len(prefix) :]
            if not relative or relative.startswith("/") or ".." in relative.split("/"):
                raise RuntimeError(
                    f"unsafe source archive path for {package['name']} {package['version']}: {member.name}"
                )
            stream = crate.extractfile(member)
            if stream is None:
                raise RuntimeError(
                    f"cannot read source archive member for {package['name']} {package['version']}: {member.name}"
                )
            files[relative] = stream.read()
    if "Cargo.toml" not in files:
        raise RuntimeError(f"source archive has no Cargo.toml: {package['name']} {package['version']}")
    CRATE_CONTENT_CACHE[key] = (archive, files)
    return archive, files


def generate_target(target: str, destination: Path, lock_hash: str, lock_packages: dict) -> None:
    metadata = cargo_metadata(target)
    packages = select_release_packages(metadata, cargo_tree_identities(target))
    roots = [
        package
        for package in metadata["packages"]
        if package["name"] == "tdmcp-daemon" and package["source"] is None
    ]
    if len(roots) != 1:
        raise RuntimeError("workspace metadata lacks a unique tdmcp-daemon package")
    root_package = roots[0]
    workspace_manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    root_repository = workspace_manifest["workspace"]["package"]["repository"].removesuffix(".git").rstrip("/")
    root_source_url = f"{root_repository}/archive/refs/tags/v{root_package['version']}.tar.gz"
    target_root = destination / target
    licenses_root = target_root / "licenses"
    sources_root = target_root / "sources"
    licenses_root.mkdir(parents=True)
    sources_root.mkdir()

    records = []
    notice_lines = [
        "td-mcp-rs third-party notices",
        "================================",
        "",
        f"Release target: {target}",
        f"Cargo.lock SHA-256: {lock_hash}",
        "",
        "This inventory is generated from the locked tdmcp-daemon release graph",
        "with the gui feature enabled. License and notice texts are stored in",
        "the adjacent licenses/ directory under content-addressed names.",
        "",
    ]
    source_records = []

    for package in packages:
        source = package["source"]
        lock_key = (package["name"], package["version"], source)
        locked = lock_packages.get(lock_key)
        if locked is None:
            raise RuntimeError(f"metadata package missing from Cargo.lock: {lock_key}")
        checksum = locked.get("checksum")
        if source.startswith("registry+") and not checksum:
            raise RuntimeError(f"registry package has no Cargo.lock checksum: {lock_key}")
        if not source.startswith("registry+"):
            raise RuntimeError(f"unsupported non-registry third-party source: {lock_key}")
        archive, crate_files = verified_crate_contents(package, checksum)

        packaged_files = []
        for original_path, content, origin in notice_files(package, crate_files):
            digest = sha256_bytes(content)
            packaged_path = f"licenses/{digest}.txt"
            output = target_root / packaged_path
            if output.exists() and output.read_bytes() != content:
                raise RuntimeError(f"SHA-256 collision while writing {packaged_path}")
            output.write_bytes(content)
            packaged_files.append(
                {
                    "originalPath": original_path,
                    "origin": origin,
                    "packagedPath": packaged_path,
                    "sha256": digest,
                }
            )

        expression = package.get("license") or "NOASSERTION"
        record = {
            "name": package["name"],
            "version": package["version"],
            "license": expression,
            "authors": package.get("authors") or [],
            "repository": package.get("repository"),
            "source": source,
            "sourceUrl": registry_download_url(package),
            "cargoLockChecksum": checksum,
            "licenseFiles": packaged_files,
        }
        component_note = COMPONENT_LICENSE_NOTES.get((package["name"], package["version"]))
        if component_note:
            record["componentLicenseNote"] = component_note
        source_reason = SOURCE_ARCHIVE_REASONS.get((package["name"], package["version"]))
        if source_reason:
            source_name = f"{package['name']}-{package['version']}.crate"
            shutil.copyfile(archive, sources_root / source_name)
            record["bundledSource"] = {
                "path": f"sources/{source_name}",
                "sha256": checksum,
                "modifiedByTdmcp": False,
                "reason": source_reason,
            }
            source_records.append(record)
        records.append(record)

        notice_lines.extend(
            [
                f"{package['name']} {package['version']}",
                f"  License: {expression}",
                f"  Source: {registry_download_url(package)}",
                f"  Cargo.lock checksum: {checksum}",
            ]
        )
        if package.get("authors"):
            notice_lines.append(f"  Authors: {', '.join(package['authors'])}")
        if package.get("repository"):
            notice_lines.append(f"  Repository: {package['repository']}")
        if component_note:
            notice_lines.append(f"  Component terms: {component_note}")
        for item in packaged_files:
            notice_lines.append(
                f"  Notice: {item['packagedPath']} "
                f"(upstream {item['originalPath']}; {item['origin']})"
            )
        if record.get("bundledSource"):
            notice_lines.append(f"  Complete source: {record['bundledSource']['path']}")
        notice_lines.append("")

    inventory = {
        "schemaVersion": 1,
        "target": target,
        "cargoLockSha256": lock_hash,
        "rootPackage": "tdmcp-daemon",
        "rootVersion": root_package["version"],
        "rootSourceUrl": root_source_url,
        "features": ["gui"],
        "dependencyKinds": ["normal"],
        "packages": records,
    }
    (target_root / "inventory.json").write_text(
        json.dumps(inventory, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n"
    )
    (target_root / "THIRD_PARTY_NOTICES.txt").write_text(
        "\n".join(notice_lines), encoding="utf-8", newline="\n"
    )

    offer_lines = [
        "# Third-party source availability and relinking",
        "",
        f"Release target: `{target}`  ",
        f"Cargo.lock SHA-256: `{lock_hash}`",
        "",
        "`inventory.json` records the exact version, registry identity, Cargo.lock",
        "checksum, upstream source URL, and preserved notice files for every",
        "third-party package in this release graph.",
        "",
        "## Corresponding dependency source",
        "",
    ]
    for record in source_records:
        bundled = record["bundledSource"]
        offer_lines.extend(
            [
                f"### {record['name']} {record['version']}",
                "",
                f"Reason: {bundled['reason']}.",
                "",
                "td-mcp-rs uses the dependency files exactly as published; this project",
                "has not modified those dependency files. The complete corresponding",
                f"Source Code Form is included at `{bundled['path']}` and its SHA-256",
                f"is `{bundled['sha256']}`, matching the Cargo.lock registry checksum.",
                f"The same source is available from {record['sourceUrl']}.",
                "The applicable license texts are identified in `inventory.json` and",
                "the adjacent `THIRD_PARTY_NOTICES.txt`.",
                "",
            ]
        )

    wayland = next(
        (record for record in source_records if record["name"] == "wayland-protocols-plasma"),
        None,
    )
    if wayland is not None:
        bundled = wayland["bundledSource"]
        offer_lines.extend(
            [
                "## Rebuild and relink the LGPL-covered Plasma protocol code",
                "",
                "The vendored Plasma protocol XML includes files under",
                "LGPL-2.1-or-later. Before binary distribution, the complete application",
                f"source corresponding to the tagged {root_package['version']} release must be published at:",
                "",
                root_source_url,
                "",
                "The td-mcp-rs MIT terms do not prohibit reverse engineering or",
                "modification for debugging changes to that covered code. To rebuild a",
                "modified executable:",
                "",
                f"1. Obtain and extract the matching td-mcp-rs source archive above.",
                f"2. Extract `{bundled['path']}` into, for example,",
                f"   `vendor/wayland-protocols-plasma-{wayland['version']}`.",
                "3. Modify the covered protocol/source files as desired.",
                "4. Add this local override to the extracted td-mcp-rs `Cargo.toml`:",
                "",
                "   ```toml",
                "   [patch.crates-io]",
                f"   wayland-protocols-plasma = {{ path = \"vendor/wayland-protocols-plasma-{wayland['version']}\" }}",
                "   ```",
                "",
                "5. Build and relink the executable (the local patch intentionally",
                "   changes the locked registry source identity):",
                "",
                "   ```sh",
                f"   cargo build --release -p tdmcp-daemon --features gui --target {target}",
                "   ```",
                "",
                "The release publisher must verify that the tag URL identifies the exact",
                "source revision used for the binary and remains available with the",
                "distributed release. A rehearsal or untagged local package does not by",
                "itself prove that final publisher-side requirement.",
                "",
            ]
        )
    (target_root / "SOURCE_OFFER.md").write_text(
        "\n".join(offer_lines) + "\n", encoding="utf-8", newline="\n"
    )


def tree_snapshot(root: Path) -> dict[str, bytes]:
    if not root.is_dir():
        return {}
    return {
        path.relative_to(root).as_posix(): path.read_bytes()
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def generate(*, check: bool) -> None:
    initial_lock_hash, lock_packages = load_lock()
    with tempfile.TemporaryDirectory(prefix="tdmcp-third-party-") as temporary:
        candidate = Path(temporary) / "generated"
        for target in TARGETS:
            print(f"Third-party inventory: {target}", flush=True)
            generate_target(target, candidate, initial_lock_hash, lock_packages)
        final_lock_hash, _ = load_lock()
        if final_lock_hash != initial_lock_hash:
            raise RuntimeError("Cargo.lock changed during notice generation; retry from stable state")

        wanted = tree_snapshot(candidate)
        current = tree_snapshot(OUTPUT_ROOT)
        if check:
            missing = sorted(wanted.keys() - current.keys())
            extra = sorted(current.keys() - wanted.keys())
            changed = sorted(path for path in wanted.keys() & current.keys() if wanted[path] != current[path])
            if missing or extra or changed:
                details = []
                if missing:
                    details.append("missing: " + ", ".join(missing[:10]))
                if extra:
                    details.append("extra: " + ", ".join(extra[:10]))
                if changed:
                    details.append("changed: " + ", ".join(changed[:10]))
                raise RuntimeError(
                    "third-party notices drifted from Cargo.lock/release graphs ("
                    + "; ".join(details)
                    + "); run python scripts/generate_third_party_notices.py"
                )
            print("Third-party notices match Cargo.lock and all release graphs")
            return

        if OUTPUT_ROOT.exists():
            shutil.rmtree(OUTPUT_ROOT)
        OUTPUT_ROOT.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(candidate, OUTPUT_ROOT)
        print(f"Wrote {len(wanted)} files under {OUTPUT_ROOT.relative_to(ROOT)}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Fail if checked-in notices differ")
    arguments = parser.parse_args()
    try:
        generate(check=arguments.check)
    except (OSError, RuntimeError, subprocess.TimeoutExpired, tarfile.TarError, tomllib.TOMLDecodeError) as error:
        parser.exit(1, f"third-party notice generation failed: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
