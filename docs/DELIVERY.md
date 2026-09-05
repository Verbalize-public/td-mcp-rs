# Packaging and releases

One binary contains the daemon, MCP server, desktop UI, bridge package,
diagnostics, skills, and bootstrap assets. There is no separate GUI executable.
Use `--no-gui` at runtime or `--no-default-features` for a headless build.

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
Archives include the executable and project `LICENSE`; Windows setup and the
macOS app bundle preserve that license. The daemon also extracts it to its
data directory when installed from a standalone binary. Use `package_smoke.py --distribution`
on an extracted archive to check it. Third-party notice coverage still needs
review before publication; see [current limitations](OPEN_WORK.md).

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

Bootstrap changes need [live TD repacking](../scripts/pack_bootstrap_tox.md).
Skill changes need [checked-in rendering](../skills/README.md).
