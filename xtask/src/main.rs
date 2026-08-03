//! xtask helpers for td-mcp-rs packaging.

#![allow(clippy::exit, reason = "process boundary")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

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
    }
}

fn workspace_root() -> Result<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .context("resolve workspace root")
}

fn release_binary_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

fn release_binary_path(workspace: &Path, base: &str) -> PathBuf {
    workspace
        .join("target/release")
        .join(release_binary_name(base))
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

/// Always rebuild release `tdmcp-daemon` with the `gui` feature so `dist` never
/// ships a stale headless binary left over from `--no-default-features`.
fn build_release_daemon_with_gui(workspace: &Path) -> Result<PathBuf> {
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "-p",
            "tdmcp-daemon",
            "--features",
            "gui",
        ])
        .current_dir(workspace)
        .status()
        .context("cargo build --release -p tdmcp-daemon --features gui")?;
    if !status.success() {
        bail!("cargo build --release -p tdmcp-daemon --features gui failed");
    }
    let src = release_binary_path(workspace, "tdmcp-daemon");
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
            .current_dir(workspace)
            .status()
    }
    .context("run kill-daemons script")?;
    if !status.success() {
        bail!("kill-daemons failed");
    }
    Ok(())
}

fn dist(out: PathBuf) -> Result<()> {
    let workspace = workspace_root()?;
    let out_dir = if out.is_absolute() {
        out
    } else {
        workspace.join(out)
    };

    // Unlock release/dist binaries before rebuild (leftover mcp shims hold locks).
    kill_workspace_daemons(&workspace)?;

    let daemon_src = build_release_daemon_with_gui(&workspace)?;
    let daemon_dest = copy_binary(&daemon_src, &out_dir)?;
    println!("{}", daemon_dest.display());
    Ok(())
}
