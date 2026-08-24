//! xtask helpers for td-mcp-rs packaging.

#![allow(clippy::exit, reason = "process boundary")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "xtask")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the local quality gate (`scripts/check`).
    Check,
    /// Assemble release tree (daemon binary; assets embedded).
    Dist {
        #[arg(long, default_value = "target/dist")]
        out: PathBuf,
    },
    /// Build the release daemon and package a named archive + SHA256SUMS.
    ///
    /// This is the same entrypoint CI uses (`cargo run -p xtask -- package`),
    /// so local artifacts are byte-for-byte the pipeline's shape.
    Package {
        /// Target triple (defaults to the host via `rustc -vV`).
        #[arg(long)]
        target: Option<String>,
        #[arg(long, default_value = "target/dist")]
        out: PathBuf,
    },
    /// Bump `[workspace.package] version`, write a CHANGELOG section from
    /// conventional commits since the last `v*` tag, commit and tag locally.
    ///
    /// Never pushes: pushing the tag is what fires the release pipeline and
    /// stays an explicit human decision.
    Release {
        /// Semver bump level.
        #[arg(default_value = "patch")]
        level: Level,
        /// Print everything that would happen; write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Record the source hash of `bridge/bootstrap.py` + `bridge/tox_callbacks.py`
    /// that `crates/tdmcp-daemon/embedded/bootstrap.tox` was packed from.
    ///
    /// Run this immediately after repacking the tox (see
    /// `scripts/pack_bootstrap_tox.md`) — it is the other half of the
    /// `bootstrap_tox_matches_packed_source_hash` test in
    /// `crates/tdmcp-daemon/src/install.rs`, which fails the build if the
    /// two `.py` sources drift from the last-packed `.tox` without anyone
    /// noticing (the `.tox` itself is an opaque TD binary format — nothing
    /// can diff its contents against source outside of TD).
    StampTox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Level {
    Patch,
    Minor,
    Major,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Check => {
            let status = if cfg!(windows) {
                Command::new("pwsh")
                    .args(["-File", "scripts/check.ps1"])
                    .status()
            } else {
                Command::new("bash").args(["scripts/check.sh"]).status()
            }
            .context("run check script")?;
            if !status.success() {
                bail!("check failed");
            }
            Ok(())
        }
        Commands::Dist { out } => dist(out),
        Commands::Package { target, out } => package(target, out),
        Commands::Release { level, dry_run } => release(level, dry_run),
        Commands::StampTox => stamp_tox(),
    }
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

fn workspace_root() -> Result<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .context("resolve workspace root")?;
    // canonicalize() yields a verbatim `\\?\C:\...` path on Windows; shelled
    // out helpers (Compress-Archive, Join-Path) reject that prefix.
    let text = path.as_os_str().to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(stripped) => Ok(PathBuf::from(stripped.to_owned())),
        None => Ok(path),
    }
}

fn release_binary_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

fn release_dir(workspace: &Path, target: Option<&str>) -> PathBuf {
    match target {
        Some(triple) => workspace.join("target").join(triple).join("release"),
        None => workspace.join("target/release"),
    }
}

fn copy_binary(src: &Path, out_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;
    let file_name = src
        .file_name()
        .context("release binary path has no file name")?;
    let dest = out_dir.join(file_name);
    fs::copy(src, &dest).with_context(|| format!("copy {} → {}", src.display(), dest.display()))?;
    Ok(dest)
}

/// Always rebuild release `tdmcp-daemon` with the `gui` feature so `dist`
/// never ships a stale headless binary left over from `--no-default-features`.
fn build_release_daemon_with_gui(workspace: &Path, target: Option<&str>) -> Result<PathBuf> {
    let mut args = vec![
        "build".to_string(),
        "--release".to_string(),
        "-p".to_string(),
        "tdmcp-daemon".to_string(),
        "--features".to_string(),
        "gui".to_string(),
    ];
    if let Some(triple) = target {
        args.push("--target".to_string());
        args.push(triple.to_string());
    }
    let status = Command::new("cargo")
        .args(&args)
        .current_dir(workspace)
        .status()
        .with_context(|| format!("cargo {}", args.join(" ")))?;
    if !status.success() {
        bail!("cargo {} failed", args.join(" "));
    }
    let src = release_dir(workspace, target).join(release_binary_name("tdmcp-daemon"));
    if !src.is_file() {
        bail!("release binary missing after build: {}", src.display());
    }
    Ok(src)
}

/// Soft-stop + force-kill workspace `tdmcp-daemon` processes locking
/// `target/release` / `target/dist` so cargo can overwrite the binary.
fn kill_workspace_daemons(workspace: &Path) -> Result<()> {
    let status = if cfg!(windows) {
        Command::new("pwsh")
            .args(["-File", "scripts/kill-daemons.ps1"])
            .current_dir(workspace)
            .status()
    } else {
        Command::new("bash")
            .args(["scripts/kill-daemons.sh"])
            .status()
    }
    .context("run kill-daemons script")?;
    if !status.success() {
        bail!("kill-daemons failed");
    }
    Ok(())
}

fn git_output(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .with_context(|| format!("git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// dist
// ---------------------------------------------------------------------------

fn dist(out: PathBuf) -> Result<()> {
    let workspace = workspace_root()?;
    let out_dir = resolve_out_dir(&workspace, &out);

    // Unlock release/dist binaries before rebuild (leftover mcp shims hold locks).
    kill_workspace_daemons(&workspace)?;

    let daemon_src = build_release_daemon_with_gui(&workspace, None)?;
    let daemon_dest = copy_binary(&daemon_src, &out_dir)?;
    println!("{}", daemon_dest.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// package
// ---------------------------------------------------------------------------

/// Archive file name for a packaged daemon: `tdmcp-rs-{version}-{target}` plus
/// a platform extension (`.zip` on Windows hosts, `.tar.gz` elsewhere).
fn artifact_stem(version: &str, target: &str) -> String {
    format!("tdmcp-rs-{version}-{target}")
}

fn host_target() -> Result<String> {
    let out = Command::new("rustc")
        .arg("-vV")
        .output()
        .context("run rustc -vV")?;
    if !out.status.success() {
        bail!("rustc -vV failed");
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(host) = line.strip_prefix("host: ") {
            return Ok(host.trim().to_owned());
        }
    }
    bail!("rustc -vV output missing 'host:' line")
}

fn package(target: Option<String>, out: PathBuf) -> Result<()> {
    let workspace = workspace_root()?;
    let triple = match target {
        Some(t) => t,
        None => host_target()?,
    };
    let out_dir = resolve_out_dir(&workspace, &out);
    fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;

    kill_workspace_daemons(&workspace)?;
    build_release_daemon_with_gui(&workspace, Some(&triple))?;

    let bin_src = release_dir(&workspace, Some(&triple)).join(release_binary_name("tdmcp-daemon"));
    if !bin_src.is_file() {
        bail!("built binary missing: {}", bin_src.display());
    }

    let toml = fs::read_to_string(workspace.join("Cargo.toml"))
        .with_context(|| format!("read {}", workspace.join("Cargo.toml").display()))?;
    let version = parse_workspace_version(&toml)?;

    let stem = artifact_stem(&version, &triple);
    let archive = stage_and_compress(&bin_src, &out_dir, &stem)?;
    println!("{}", archive.display());

    let sums = rewrite_sha256_sums(&out_dir)?;
    if let Some(sums_path) = sums {
        println!("{}", sums_path.display());
    }
    Ok(())
}

fn resolve_out_dir(workspace: &Path, out: &Path) -> PathBuf {
    if out.is_absolute() {
        out.to_path_buf()
    } else {
        workspace.join(out)
    }
}

/// Copy the binary into a staging dir (archive root layout: one `tdmcp-daemon`)
/// and compress it with the platform's built-in archiver.
///
/// No zip/tar crates on purpose: GitHub runners and dev boxes both ship
/// `Compress-Archive` / bsdtar, and a dependency-free xtask builds fast in CI.
fn stage_and_compress(bin_src: &Path, out_dir: &Path, stem: &str) -> Result<PathBuf> {
    let stage = out_dir.join(format!(".stage-{stem}"));
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage)
        .with_context(|| format!("create staging dir {}", stage.display()))?;

    let staged_bin = stage.join(release_binary_name("tdmcp-daemon"));
    fs::copy(bin_src, &staged_bin)
        .with_context(|| format!("copy {} → {}", bin_src.display(), staged_bin.display()))?;

    let is_windows = cfg!(windows);
    let archive_name = if is_windows {
        format!("{stem}.zip")
    } else {
        format!("{stem}.tar.gz")
    };
    let archive = out_dir.join(&archive_name);
    let _ = fs::remove_file(&archive);

    let status = if is_windows {
        // `\*` — archive the CONTENTS at zip root, not the staging folder itself.
        Command::new("pwsh")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "Compress-Archive -Path '{}\\*' -DestinationPath '{}' -Force",
                    stage.display(),
                    archive.display()
                ),
            ])
            .status()
            .context("Compress-Archive")?
    } else {
        let file_name = staged_bin
            .file_name()
            .context("staged binary has no file name")?
            .to_owned();
        Command::new("tar")
            .args(["-czf"])
            .arg(&archive)
            .arg("-C")
            .arg(&stage)
            .arg(&file_name)
            .status()
            .context("tar -czf")?
    };
    if !status.success() {
        bail!("archiving {} failed", archive.display());
    }
    let _ = fs::remove_dir_all(&stage);

    if !archive.is_file() {
        bail!("archive missing after compression: {}", archive.display());
    }
    Ok(archive)
}

/// SHA-256 hex digest of one file, shelling out to the platform tool
/// (`Get-FileHash` on Windows, `sha256sum`/`shasum -a 256` elsewhere).
fn sha256_hex(path: &Path) -> Result<String> {
    let hex = if cfg!(windows) {
        let out = Command::new("pwsh")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-FileHash -Algorithm SHA256 '{}').Hash", path.display()),
            ])
            .output()
            .context("Get-FileHash")?;
        if !out.status.success() {
            bail!("Get-FileHash failed for {}", path.display());
        }
        String::from_utf8_lossy(&out.stdout).trim().to_lowercase()
    } else {
        let tools: [&[&str]; 2] = [&["sha256sum"], &["shasum", "-a", "256"]];
        let mut last_err = None;
        for tool in tools {
            match Command::new(tool[0]).args(&tool[1..]).arg(path).output() {
                Ok(out) if out.status.success() => {
                    let text = String::from_utf8_lossy(&out.stdout);
                    let hash = text
                        .split_whitespace()
                        .next()
                        .context("sha256 output empty")?
                        .to_owned();
                    return Ok(hash);
                }
                Ok(out) => {
                    last_err = Some(format!(
                        "{} exited {}: {}",
                        tool[0],
                        out.status,
                        String::from_utf8_lossy(&out.stderr).trim()
                    ));
                }
                Err(e) => last_err = Some(format!("{}: {e}", tool[0])),
            }
        }
        bail!(
            "no sha256 tool available (tried sha256sum, shasum): {}",
            last_err.unwrap_or_default()
        );
    };
    Ok(hex)
}

/// Recompute `SHA256SUMS.txt` over every `tdmcp-rs-*.{zip,tar.gz}` archive in
/// `out_dir`. Returns the sums path, or `None` when nothing was packaged.
fn rewrite_sha256_sums(out_dir: &Path) -> Result<Option<PathBuf>> {
    let mut archives: Vec<PathBuf> = fs::read_dir(out_dir)
        .with_context(|| format!("read {}", out_dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(is_packaged_archive)
        })
        .collect();
    archives.sort();
    if archives.is_empty() {
        return Ok(None);
    }

    let mut lines = Vec::new();
    for archive in &archives {
        let hash = sha256_hex(archive)?;
        let name = archive
            .file_name()
            .and_then(|n| n.to_str())
            .context("archive name not utf-8")?
            .to_owned();
        lines.push(format!("{hash}  {name}"));
    }
    let sums_path = out_dir.join("SHA256SUMS.txt");
    fs::write(&sums_path, lines.join("\n") + "\n")
        .with_context(|| format!("write {}", sums_path.display()))?;
    Ok(Some(sums_path))
}

/// An archive produced by this packager: versioned name with zip/tar.gz ext.
fn is_packaged_archive(file_name: &str) -> bool {
    file_name.starts_with("tdmcp-rs-")
        && (file_name.ends_with(".zip") || file_name.ends_with(".tar.gz"))
}

// ---------------------------------------------------------------------------
// release
// ---------------------------------------------------------------------------

/// Extract the `[workspace.package] version` value from root `Cargo.toml`.
fn parse_workspace_version(content: &str) -> Result<String> {
    const SECTION: &str = "[workspace.package]";
    let section_start = content
        .find(SECTION)
        .context("Cargo.toml missing [workspace.package]")?;
    let section = &content[section_start + SECTION.len()..];
    let section_end = section.find("\n[").unwrap_or(section.len());
    for line in section[..section_end].lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("version") {
            let rest = rest.trim_start();
            if let Some(value) = rest.strip_prefix('=') {
                let value = value.trim().trim_matches('"');
                return Ok(value.to_owned());
            }
        }
    }
    bail!("[workspace.package] has no version key")
}

/// Replace only the `[workspace.package] version` value, preserving everything
/// else byte-for-byte. Assumes the canonical `version = "x.y.z"` formatting
/// this repo's root manifest uses.
fn bump_workspace_version(content: &str, new_version: &str) -> Result<String> {
    const SECTION: &str = "[workspace.package]";
    const MARKER: &str = "version = \"";
    let section_start = content
        .find(SECTION)
        .context("Cargo.toml missing [workspace.package]")?;
    let marker_at = content[section_start..]
        .find(MARKER)
        .context("[workspace.package] has no version key")?
        + section_start;
    let value_start = marker_at + MARKER.len();
    let value_end = content[value_start..]
        .find('"')
        .context("version value not terminated")?
        + value_start;
    let mut out = String::with_capacity(content.len() + 16);
    out.push_str(&content[..value_start]);
    out.push_str(new_version);
    out.push_str(&content[value_end..]);
    Ok(out)
}

/// Bump `major.minor.patch`; prerelease/build tags are rejected because the
/// release flow tags exactly `v{version}`.
fn next_version(version: &str, level: Level) -> Result<String> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 {
        bail!("unsupported version {version:?}: expected major.minor.patch");
    }
    let mut nums = [0u64; 3];
    for (slot, part) in parts.iter().enumerate() {
        nums[slot] = part
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid numeric component {part:?} in {version:?}"))?;
    }
    match level {
        Level::Patch => nums[2] += 1,
        Level::Minor => {
            nums[1] += 1;
            nums[2] = 0;
        }
        Level::Major => {
            nums[0] += 1;
            nums[1] = 0;
            nums[2] = 0;
        }
    }
    Ok(nums
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join("."))
}

/// Bucket a conventional-commit subject into a CHANGELOG group.
fn conventional_group(subject: &str) -> &'static str {
    let kind = subject.split([':', '(']).next().unwrap_or(subject).trim();
    match kind {
        "feat" => "Added",
        "fix" => "Fixed",
        "perf" => "Performance",
        _ => "Other",
    }
}

/// Render one `## vX.Y.Z — date` section, grouped Added/Fixed/Performance/Other.
fn changelog_section(tag: &str, date: &str, subjects: &[String]) -> String {
    const GROUP_ORDER: [&str; 4] = ["Added", "Fixed", "Performance", "Other"];
    let mut body = String::new();
    if subjects.is_empty() {
        body.push_str("- Initial tagged release.\n");
    } else {
        for group in GROUP_ORDER {
            let items: Vec<&String> = subjects
                .iter()
                .filter(|s| conventional_group(s) == group)
                .collect();
            if items.is_empty() {
                continue;
            }
            body.push_str(&format!("### {group}\n"));
            for item in items {
                body.push_str(&format!("- {item}\n"));
            }
            body.push('\n');
        }
    }
    format!("## {tag} — {date}\n\n{body}")
}

fn insert_changelog_section(existing: Option<&str>, section: &str) -> String {
    match existing {
        // Insert directly under the `# Changelog` header, newest section first.
        Some(text) if text.starts_with("# Changelog") => {
            let header_end = text.find('\n').map(|i| i + 1).unwrap_or(text.len());
            let (header, rest) = text.split_at(header_end);
            format!("{header}\n{section}{}", rest.trim_start_matches('\n'))
        }
        Some(text) => format!("{section}{text}"),
        None => format!("# Changelog\n\n{section}"),
    }
}

fn release(level: Level, dry_run: bool) -> Result<()> {
    let workspace = workspace_root()?;
    let cargo_toml = workspace.join("Cargo.toml");
    let content = fs::read_to_string(&cargo_toml)
        .with_context(|| format!("read {}", cargo_toml.display()))?;
    let current = parse_workspace_version(&content)?;
    let next = next_version(&current, level)?;
    let tag = format!("v{next}");

    let last_tag = git_output(&workspace, &["tag", "--list", "v*", "--sort=-v:refname"])?
        .lines()
        .next()
        .map(str::to_owned);
    let log_range: Vec<String> = match &last_tag {
        Some(t) => vec![t.clone(), "..HEAD".to_string()],
        None => vec!["HEAD".to_string()],
    };
    let mut log_args = vec!["log", "--pretty=%s"];
    log_args.extend(log_range.iter().map(String::as_str));
    let subjects: Vec<String> = git_output(&workspace, &log_args)?
        .lines()
        .take(300)
        .map(str::to_owned)
        .collect();

    let date = Utc::now().date_naive().to_string();
    let section = changelog_section(&tag, &date, &subjects);

    println!("release: {current} → {next} ({level:?})");
    println!("{section}");
    if dry_run {
        println!("dry-run: no files touched, no commit/tag created");
        return Ok(());
    }

    let dirty = git_output(&workspace, &["status", "--porcelain"])?;
    if !dirty.trim().is_empty() {
        bail!("worktree dirty — commit or stash before releasing:\n{dirty}");
    }
    if !git_output(&workspace, &["tag", "--list", &tag])?
        .trim()
        .is_empty()
    {
        bail!("tag {tag} already exists");
    }

    let new_content = bump_workspace_version(&content, &next)?;
    fs::write(&cargo_toml, new_content)
        .with_context(|| format!("write {}", cargo_toml.display()))?;

    let changelog_path = workspace.join("CHANGELOG.md");
    let existing = fs::read_to_string(&changelog_path).ok();
    let updated = insert_changelog_section(existing.as_deref(), &section);
    fs::write(&changelog_path, updated)
        .with_context(|| format!("write {}", changelog_path.display()))?;

    let status = Command::new("cargo")
        .args(["update", "-w"])
        .current_dir(&workspace)
        .status()
        .context("cargo update -w")?;
    if !status.success() {
        bail!("cargo update -w failed");
    }

    for file in ["Cargo.toml", "Cargo.lock", "CHANGELOG.md"] {
        let st = Command::new("git")
            .args(["add", file])
            .current_dir(&workspace)
            .status()
            .with_context(|| format!("git add {file}"))?;
        if !st.success() {
            bail!("git add {file} failed");
        }
    }
    let commit_msg = format!("chore(release): {tag}");
    let st = Command::new("git")
        .args(["commit", "-m", &commit_msg])
        .current_dir(&workspace)
        .status()
        .context("git commit")?;
    if !st.success() {
        bail!("git commit failed");
    }
    let st = Command::new("git")
        .args(["tag", "-a", &tag, "-m", &tag])
        .current_dir(&workspace)
        .status()
        .context("git tag")?;
    if !st.success() {
        bail!("git tag {tag} failed");
    }

    println!("created commit {commit_msg} and annotated tag {tag}");
    println!("not pushed. when ready: git push && git push origin {tag}");
    Ok(())
}

// ---------------------------------------------------------------------------
// stamp-tox
// ---------------------------------------------------------------------------

/// Deterministic, dependency-free content hash (FNV-1a, 64-bit) — this is a
/// drift check, not a security boundary, so stdlib's `DefaultHasher` (whose
/// algorithm stability across toolchains isn't guaranteed) and a real crypto
/// hash crate are both more than this needs.
fn fnv1a(chunks: &[&[u8]]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET;
    for chunk in chunks {
        for &byte in *chunk {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    hash
}

fn stamp_tox() -> Result<()> {
    let workspace = workspace_root()?;
    let bootstrap_py = workspace.join("bridge/bootstrap.py");
    let callbacks_py = workspace.join("bridge/tox_callbacks.py");
    let hash_path = workspace.join("crates/tdmcp-daemon/embedded/bootstrap.tox.source-hash");

    let bootstrap =
        fs::read(&bootstrap_py).with_context(|| format!("read {}", bootstrap_py.display()))?;
    let callbacks =
        fs::read(&callbacks_py).with_context(|| format!("read {}", callbacks_py.display()))?;
    let hash = fnv1a(&[&bootstrap, &callbacks]);

    fs::write(&hash_path, format!("{hash:016x}\n"))
        .with_context(|| format!("write {}", hash_path.display()))?;
    println!(
        "stamped {} ({hash:016x}) from bootstrap.py + tox_callbacks.py",
        hash_path.display()
    );
    println!(
        "reminder: this only records that the .tox matches source — it does NOT repack. \
         If you changed bootstrap.py or tox_callbacks.py, you must have already re-run the \
         live-TD packing script in scripts/pack_bootstrap_tox.md and saved over \
         crates/tdmcp-daemon/embedded/bootstrap.tox before stamping."
    );
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn artifact_stem_format() {
        assert_eq!(
            artifact_stem("0.1.4", "x86_64-pc-windows-msvc"),
            "tdmcp-rs-0.1.4-x86_64-pc-windows-msvc"
        );
    }

    #[test]
    fn next_version_levels() {
        assert_eq!(next_version("0.1.3", Level::Patch).unwrap(), "0.1.4");
        assert_eq!(next_version("0.1.3", Level::Minor).unwrap(), "0.2.0");
        assert_eq!(next_version("0.1.3", Level::Major).unwrap(), "1.0.0");
        assert_eq!(next_version("0.9.9", Level::Minor).unwrap(), "0.10.0");
    }

    #[test]
    fn next_version_rejects_non_semver_core() {
        for bad in ["1.2", "1.2.3.4", "01.02.03x", "1.2.3-rc1", "", "a.b.c"] {
            assert!(next_version(bad, Level::Patch).is_err(), "{bad:?}");
        }
    }

    const SAMPLE_TOML: &str = "[workspace]\nresolver = \"2\"\n\n[workspace.package]\nversion = \"0.1.3\"\nedition = \"2021\"\nlicense = \"MIT\"\nrust-version = \"1.88\"\nrepository = \"https://github.com/example/x\"\n\n[dependencies]\nanyhow = \"1\"\n";

    #[test]
    fn parse_workspace_version_finds_key() {
        assert_eq!(parse_workspace_version(SAMPLE_TOML).unwrap(), "0.1.3");
    }

    #[test]
    fn parse_workspace_version_errors_without_section() {
        assert!(parse_workspace_version("[package]\nversion = \"1.0.0\"\n").is_err());
    }

    #[test]
    fn bump_workspace_version_touches_only_the_line() {
        let bumped = bump_workspace_version(SAMPLE_TOML, "0.2.0").unwrap();
        assert!(bumped.contains("version = \"0.2.0\""));
        assert!(!bumped.contains("0.1.3"));
        // Everything else byte-identical.
        let strip = |s: &str, v: &str| s.replace(&format!("version = \"{v}\""), "");
        assert_eq!(
            strip(&bumped, "0.2.0"),
            strip(SAMPLE_TOML, "0.1.3"),
            "non-version content must be preserved"
        );
        assert_eq!(
            parse_workspace_version(&bumped).unwrap(),
            "0.2.0",
            "bumped toml still parses"
        );
    }

    #[test]
    fn bump_workspace_version_errors_when_missing() {
        assert!(bump_workspace_version("[package]\nversion = \"1.0.0\"\n", "1.0.1").is_err());
    }

    #[test]
    fn conventional_groups_map_by_kind() {
        assert_eq!(conventional_group("feat(gui): add panel"), "Added");
        assert_eq!(conventional_group("fix: leak"), "Fixed");
        assert_eq!(conventional_group("perf: faster loop"), "Performance");
        assert_eq!(conventional_group("docs: readme"), "Other");
        assert_eq!(conventional_group("chore(release): v1"), "Other");
        assert_eq!(conventional_group("random commit"), "Other");
    }

    #[test]
    fn changelog_section_groups_in_order() {
        let subjects: Vec<String> = ["fix: b", "feat: a", "docs: c", "feat(scope): d", "perf: e"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let section = changelog_section("v0.2.0", "2026-08-24", &subjects);
        assert!(section.starts_with("## v0.2.0 — 2026-08-24\n"));
        let added = section.find("### Added").unwrap();
        let fixed = section.find("### Fixed").unwrap();
        let perf = section.find("### Performance").unwrap();
        let other = section.find("### Other").unwrap();
        assert!(added < fixed && fixed < perf && perf < other);
        assert!(section.contains("- feat: a"));
        assert!(section.contains("- feat(scope): d"));
        assert!(section.contains("- docs: c"));
    }

    #[test]
    fn changelog_section_initial_release_when_no_commits() {
        let section = changelog_section("v0.1.0", "2026-08-24", &[]);
        assert!(section.contains("- Initial tagged release."));
    }

    #[test]
    fn insert_changelog_creates_file_with_header() {
        let out = insert_changelog_section(None, "## v1.0.0 — d\n\n- x\n");
        assert_eq!(out, "# Changelog\n\n## v1.0.0 — d\n\n- x\n");
    }

    #[test]
    fn insert_changelog_prepends_after_header() {
        let existing = "# Changelog\n\n## v0.9.0 — d\n\n- old\n";
        let out = insert_changelog_section(Some(existing), "## v1.0.0 — d\n\n- new\n");
        assert!(out.starts_with("# Changelog\n\n## v1.0.0 — d\n"));
        assert!(out.contains("## v0.9.0"));
        assert!(out.find("v1.0.0").unwrap() < out.find("v0.9.0").unwrap());
    }

    #[test]
    fn insert_changelog_prepends_to_headerless_file() {
        let out = insert_changelog_section(Some("## v0.9.0 — d\n\n- old\n"), "## v1.0.0\n");
        assert!(out.starts_with("## v1.0.0"));
        assert!(out.ends_with("## v0.9.0 — d\n\n- old\n"));
    }

    #[test]
    fn is_packaged_archive_filter() {
        assert!(is_packaged_archive(
            "tdmcp-rs-0.1.3-x86_64-pc-windows-msvc.zip"
        ));
        assert!(is_packaged_archive(
            "tdmcp-rs-0.1.3-aarch64-apple-darwin.tar.gz"
        ));
        assert!(!is_packaged_archive("SHA256SUMS.txt"));
        assert!(!is_packaged_archive("other.zip"));
        assert!(!is_packaged_archive("tdmcp-rs-0.1.3.tar.bz2"));
        assert!(!is_packaged_archive("prefix-tdmcp-rs-0.1.3.zip"));
    }
}
