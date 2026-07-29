//! xtask helpers for td-mcp-rs packaging.

#![allow(clippy::exit, reason = "process boundary")]

use std::path::PathBuf;
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
    /// Placeholder for release assembly (P2).
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
        Commands::Dist { out } => {
            println!("dist placeholder → {}", out.display());
            println!("(Gate P2: copy daemon+gui binaries, bridge/, catalog, tox)");
            Ok(())
        }
    }
}
