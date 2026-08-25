//! Injectable process runner. Production spawns without a shell; tests script
//! outputs (`FakeOfficialRunner`) so CI never needs TouchDesigner.
//!
//! Exit-code semantics live with the CALLER (filesystem evidence law) — the
//! runner only reports what happened.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What a tool invocation produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    /// Process exit code (`-1` when the platform gave none).
    pub code: i32,
    /// Captured stdout (lossy UTF-8).
    pub stdout: String,
    /// Captured stderr (lossy UTF-8).
    pub stderr: String,
}

/// Seam for invoking official tools.
pub trait CommandRunner: Send + Sync {
    /// Run `program` with `args`, capturing output. No shell involved.
    ///
    /// # Errors
    /// Spawn/wait IO failures (binary vanished, permission denied).
    fn run(&self, program: &Path, args: &[&std::ffi::OsStr]) -> std::io::Result<CommandOutput>;
}

/// Real process runner.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessRunner;

impl CommandRunner for ProcessRunner {
    fn run(&self, program: &Path, args: &[&std::ffi::OsStr]) -> std::io::Result<CommandOutput> {
        let out = Command::new(program).args(args).output()?;
        Ok(CommandOutput {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Scripted runner for tests/CI: pops queued outputs per call, optionally
/// simulating spawn failure.
#[derive(Debug, Default)]
pub struct FakeOfficialRunner {
    scripted: std::sync::Mutex<Vec<Result<CommandOutput, std::io::Error>>>,
    /// Every (program,args) request, in order — assertions read this afterwards.
    pub calls: std::sync::Mutex<Vec<(PathBuf, Vec<String>)>>,
}

impl FakeOfficialRunner {
    fn queue(&self) -> std::sync::MutexGuard<'_, Vec<Result<CommandOutput, std::io::Error>>> {
        // Panic-free poisoning recovery: scripted outputs stay usable.
        self.scripted.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn calls_mut(&self) -> std::sync::MutexGuard<'_, Vec<(PathBuf, Vec<String>)>> {
        self.calls.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Queue one successful output.
    pub fn push_ok(&self, code: i32, stdout: &str, stderr: &str) -> &Self {
        self.queue().push(Ok(CommandOutput {
            code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }));
        self
    }

    /// Queue one spawn failure.
    pub fn push_err(&self, err: std::io::Error) -> &Self {
        self.queue().push(Err(err));
        self
    }

    fn next_scripted(&self) -> Option<Result<CommandOutput, std::io::Error>> {
        let mut q = self.queue();
        if q.is_empty() {
            None
        } else {
            Some(q.remove(0))
        }
    }
}

impl CommandRunner for FakeOfficialRunner {
    fn run(&self, program: &Path, args: &[&std::ffi::OsStr]) -> std::io::Result<CommandOutput> {
        self.calls_mut().push((
            program.to_path_buf(),
            args.iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
        ));
        match self.next_scripted() {
            Some(r) => r,
            None => Err(std::io::Error::other(
                "FakeOfficialRunner: no scripted output",
            )),
        }
    }
}
