//! Injectable process runner. Production spawns without a shell; tests script
//! outputs (`FakeOfficialRunner`) so CI never needs TouchDesigner.
//!
//! Exit-code semantics live with the CALLER (filesystem evidence law) — the
//! runner only reports what happened.

use std::path::{Path, PathBuf};

/// Scripted side-effect: materialize artifacts a real tool would have written.
pub type RunnerEffect = Box<dyn FnOnce(&Path, &[String]) + Send>;

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
        let out = crate::wine::command_for(program, false)
            .args(args)
            .output()?;
        Ok(CommandOutput {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Scripted runner for tests/CI: pops queued outputs per call, optionally
/// simulating spawn failure and/or running a filesystem side-effect (so
/// evidence gates can be exercised without real tools).
pub struct FakeOfficialRunner {
    scripted: std::sync::Mutex<Vec<Result<CommandOutput, std::io::Error>>>,
    effects: std::sync::Mutex<Vec<Option<RunnerEffect>>>,
    /// Every (program,args) request, in order — assertions read this afterwards.
    pub calls: std::sync::Mutex<Vec<(PathBuf, Vec<String>)>>,
}

impl Default for FakeOfficialRunner {
    fn default() -> Self {
        Self {
            scripted: std::sync::Mutex::new(Vec::new()),
            effects: std::sync::Mutex::new(Vec::new()),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl FakeOfficialRunner {
    fn queue(&self) -> std::sync::MutexGuard<'_, Vec<Result<CommandOutput, std::io::Error>>> {
        // Panic-free poisoning recovery: scripted outputs stay usable.
        self.scripted.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn calls_mut(&self) -> std::sync::MutexGuard<'_, Vec<(PathBuf, Vec<String>)>> {
        self.calls.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn effects_mut(&self) -> std::sync::MutexGuard<'_, Vec<Option<RunnerEffect>>> {
        self.effects.lock().unwrap_or_else(|p| p.into_inner())
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

    /// Queue one successful output that first runs `effect` — lets tests
    /// materialize the artifacts a real tool would have written so the
    /// filesystem-evidence gates are exercised end-to-end.
    pub fn push_ok_with_effect(&self, code: i32, stderr: &str, effect: RunnerEffect) -> &Self {
        self.queue().push(Ok(CommandOutput {
            code,
            stdout: String::new(),
            stderr: stderr.to_string(),
        }));
        self.effects_mut().push(Some(effect));
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

    fn next_effect(&self) -> Option<Option<RunnerEffect>> {
        let mut e = self.effects_mut();
        if e.is_empty() {
            None
        } else {
            Some(e.remove(0))
        }
    }
}

impl CommandRunner for FakeOfficialRunner {
    fn run(&self, program: &Path, args: &[&std::ffi::OsStr]) -> std::io::Result<CommandOutput> {
        let args_owned: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        if let Some(Some(effect)) = self.next_effect() {
            effect(program, &args_owned);
        }
        self.calls_mut().push((program.to_path_buf(), args_owned));
        match self.next_scripted() {
            Some(r) => r,
            None => Err(std::io::Error::other(
                "FakeOfficialRunner: no scripted output",
            )),
        }
    }
}
