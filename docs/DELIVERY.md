# Packaging and releases

One binary contains the daemon, MCP server, desktop UI, bridge package,
diagnostics, skills, and bootstrap assets. There is no separate GUI executable.
The tray UI is on by default for every entry point, including the IDE-spawned
`mcp`; use `--no-gui` at runtime, `daemon.show_tray = false` in the config, or
`--no-default-features` for a headless build.

## Release workflow

Push `vX.Y.Z` only when it matches the workspace version and the lockfile is
committed. The [release workflow](../.github/workflows/release.yml) runs:

1. Tag/version validation, the Linux quality gate, and per-target dependency checks.
2. Builds for Linux x86_64, Windows MSVC x64, macOS ARM64, and macOS Intel.
   Windows/macOS run native workspace tests.
3. Archive extraction and an isolated real-MCP installation smoke test.
4. Windows Inno Setup installer and macOS app/DMG packaging.
5. Checksums covering archives and installers, then GitHub Release publication.

A failed check or platform build prevents publication. Artifact retention is
three days; the published release retains the final files. Public repositories
also receive build-provenance attestations.

Manually dispatch **Release** to rehearse the same native tests and packaging
without creating a tag or publishing a release. Download the resulting workflow
artifacts to verify native installation before tagging.

Ordinary branch commits do not run CI automatically. Use workflow dispatch
when needed; its optional native flag adds Windows/macOS tests. Dependency
checks run weekly and on release/rehearsal; manual Checks can opt in too.
Unresolved advisory or license-policy failures block release packaging.
See [Testing](TESTING.md) for the local gate.

## Local commands

```sh
cargo run --locked -p xtask -- package --out dist
cargo run -p xtask -- release minor --dry-run
python scripts/package_smoke.py target/debug/tdmcp-daemon
```

`xtask release patch|minor|major` updates versions/changelog, commits, and
creates a tag; it does not push. Inspect a dry run first.
Packaging uses the same xtask command locally and in CI.
Archives include the executable, project `LICENSE`, target-specific
`THIRD_PARTY_NOTICES.txt`, `SOURCE_OFFER.md`, and the referenced license/source
inventory under `third-party/`. Windows setup and the macOS app bundle preserve
all of those records. The daemon also embeds and extracts them to its data
directory when installed from a standalone binary. Use
`package_smoke.py --distribution` on an extracted archive to verify hashes and
coverage before publication.

`python scripts/generate_third_party_notices.py` derives each inventory from the
locked daemon release graph (GUI feature; normal dependencies actually shipped
in the executable) for all four release targets. Build-only tools remain covered
by cargo-deny's policy gate but are not represented as redistributed code in the
notice inventory. `--check` is part of `scripts/check_dependencies.py` and
fails on lock/version/source or notice-text drift. Published crates that omit an
inherited workspace license use pinned SPDX License List 3.27.0 texts. The
inventory also preserves nested component terms that crate-level metadata does
not expose: the selected FreeType fallback is FTL (the alternative GPLv2 text is
retained too), and Plasma protocol XML includes LGPL-2.1-or-later/MIT-CMU/BSD/
CC0 terms. MPL and LGPL-covered dependency source archives are copied exactly
from Cargo's cache and checksum-matched to Cargo.lock; the source offer states
truthfully that td-mcp-rs does not modify those dependency files and gives
rebuild/relink instructions.

Before publishing, the publisher must verify that the source archive for the
matching release tag is the exact td-mcp-rs revision used for each binary and
remains available with the release. Local generation or a rehearsal does not
prove that source-publication requirement. A new dependency, target, nested
component, or license requires regeneration and policy review.

This notice integration does not change signing: Windows publisher signing and
macOS Developer ID/notarization still require credentials.

## Installation behavior

`tdmcp-daemon install` copies a stable executable to `{dataDir}/bin/`,
records that path, and extracts embedded assets. Version changes refresh
managed assets; `--force` also refreshes a same-version build.
Configuration and customized `template.toe` are preserved.
Asset installs are serialized and prepared in a temporary directory before
replacement. Filesystem failures roll back replaced entries; if rollback itself
fails, the error identifies the retained backup. This is not a crash-atomic
transaction across the entire installation. Binary copies are staged too.

The HTTP daemon is independent of assistant stdio processes. A stdio client
ensures the daemon is running and reconnects after a restart. Each listener
has one process owner; an admin restart starts a replacement that waits for
the old PID to exit before binding.

## Platform packaging

- Windows: per-user Inno Setup installer; no administrator rights required
  for normal installation. Source builds use the MSVC toolchain.
- macOS: `packaging/macos/make_app.sh` creates an app bundle and DMG.
  New artifacts are staged and verified before replacing previous output;
  failures retain the previous bundle/DMG or identify a recovery directory.
  Without publisher credentials it uses ad-hoc signing, not notarization.
  Developer ID signing and notarization require a configured identity and
  keychain profile on the build machine; hosted CI does not provision them.
- Linux: compressed executable archive. TD itself is separate and runs
  through Wine. See [Linux/Wine](LINUX_SUPPORT.md).

Do not describe unsigned installers as signed or notarized. Publishing and
native OS installation cannot be fully proven by Linux-only local checks.

Upgrading from an earlier version: [Migrating to v0.2.0](MIGRATION.md).

Bootstrap changes need [live TD repacking](../scripts/pack_bootstrap_tox.md).
Skill changes need [checked-in rendering](../skills/README.md).

## Manual releases

Run the Release workflow on the intended ref and enter its full commit SHA.
Choose `build-only` to retain qualified artifacts without a release, `draft`
(the default) to stage one, or `publish` to publish it. The workspace version
must match its immutable `v<version>` tag; existing tags are never moved.

Shared source checks precede native qualification on Linux x64, Windows x64
MSVC, macOS ARM64 and macOS Intel. Each target runs dependency checks, native
Rust tests, packaging and an extracted-package MCP smoke test. Successful
platforms can publish independently; release notes prominently name missing
platforms and an issue records each failure. No qualified platforms means no
publication. Failed-job logs and available artifacts are retained for 90 days.

Rerun failed jobs in the same run to reuse the exact source commit. Qualification
manifests bind files and hashes to that commit. Existing archive assets must
match byte-for-byte; only the aggregate checksum file and release notes are
updated. Unsigned archives/installers need no signing credentials; macOS ad-hoc
signatures are not Developer ID signatures or notarization.
