//! Daemon-side OS popup detection + dismissal for TD processes.
//!
//! Platform backends behind one narrow `sys` facade; everything above the
//! facade (classification, dismiss ladder, budgets) is portable and
//! unit-tested without an OS. Windows ships first (user32 now, UIA content
//! fill-in next); macOS implements the same facade via CGWindowList + AX.
//!
//! # Unsafe policy (constitution carve-out)
//!
//! ALL `unsafe` lives under [`sys`] (platform FFI shim). The public API of this
//! crate is 100% safe. Every unsafe block carries a `// SAFETY:` comment.

pub mod classify;
pub mod policy;
pub mod sys;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use tdmcp_core::{DialogError, DialogSnapshot, DialogSource, DismissOutcome, PopupInfo};

#[cfg(target_os = "macos")]
use crate::sys::macos as platform;
#[cfg(all(not(windows), not(target_os = "macos")))]
use crate::sys::stub as platform;
#[cfg(windows)]
use crate::sys::windows as platform;

/// Budgets (DIALOGS.md §5.2): snapshot is on the watcher hot path.
pub const SNAPSHOT_BUDGET: Duration = Duration::from_millis(150);
/// Full-content extraction budget.
pub const DESCRIBE_BUDGET: Duration = Duration::from_millis(500);
/// Whole dismiss ladder incl. verify-gone loop.
pub const DISMISS_BUDGET: Duration = Duration::from_millis(3000);
/// Snapshot cache TTL (matches default watcher poll cadence).
pub const CACHE_TTL: Duration = Duration::from_millis(1000);
/// Verify-gone delay between polls after a dismissal attempt.
pub const VERIFY_DELAY: Duration = Duration::from_millis(300);

enum Job {
    TopLevel {
        pid: u32,
        resp: mpsc::Sender<std::io::Result<Vec<crate::sys::SysWindow>>>,
    },
    Children {
        id: String,
        resp: mpsc::Sender<Vec<crate::sys::SysControl>>,
    },
    Click {
        id: String,
        ctrl_id: i32,
        resp: mpsc::Sender<bool>,
    },
    Close {
        id: String,
        resp: mpsc::Sender<bool>,
    },
    Hung {
        id: String,
        budget_ms: u32,
        resp: mpsc::Sender<bool>,
    },
    ImageName {
        pid: u32,
        resp: mpsc::Sender<Option<String>>,
    },
}

fn spawn_worker() -> mpsc::Sender<Job> {
    let (tx, rx) = mpsc::channel::<Job>();
    let built = std::thread::Builder::new()
        .name("tdmcp-dialogs".into())
        .spawn(move || {
            // Single dedicated OS thread owns every platform call; COM init for
            // the future UIA module will live here too (DIALOGS.md §4).
            while let Ok(job) = rx.recv() {
                match job {
                    Job::TopLevel { pid, resp } => {
                        let _ = resp.send(platform::top_level_windows(pid));
                    }
                    Job::Children { id, resp } => {
                        let _ = resp.send(platform::child_controls(&id));
                    }
                    Job::Click { id, ctrl_id, resp } => {
                        let _ = resp.send(platform::post_click(&id, ctrl_id));
                    }
                    Job::Close { id, resp } => {
                        let _ = resp.send(platform::post_close(&id));
                    }
                    Job::Hung {
                        id,
                        budget_ms,
                        resp,
                    } => {
                        let _ = resp.send(platform::is_hung(&id, budget_ms));
                    }
                    Job::ImageName { pid, resp } => {
                        let _ = resp.send(platform::process_image_name(pid));
                    }
                }
            }
        });
    // On spawn failure rx drops with the rejected closure -> sends below fail
    // and every op fails open (empty snapshots / Unsupported), never panics.
    if let Err(err) = built {
        tracing::error!(%err, "dialogs worker failed to spawn - failing open");
    }
    tx
}

fn ask<T>(
    tx: &mpsc::Sender<Job>,
    make: impl FnOnce(mpsc::Sender<T>) -> Job,
    budget: Duration,
) -> Option<T> {
    let (resp, rx) = mpsc::channel::<T>();
    tx.send(make(resp)).ok()?;
    rx.recv_timeout(budget).ok()
}

/// Platform backend source: cached snapshots through the serialized worker.
///
/// Fail-open everywhere: probe timeouts degrade to empty snapshots / typed
/// errors, never block or worsen a healthy call (DIALOGS.md §7).
pub struct PlatformDialogSource {
    worker: mpsc::Sender<Job>,
    cache: std::sync::Mutex<std::collections::HashMap<u32, (Instant, DialogSnapshot)>>,
}

/// Windows alias (public API stability).
#[cfg(windows)]
pub type Win32Source = PlatformDialogSource;

/// macOS alias.
#[cfg(target_os = "macos")]
pub type MacDialogSource = PlatformDialogSource;

impl PlatformDialogSource {
    /// Create the source and its dedicated worker thread.
    pub fn new() -> Self {
        Self {
            worker: spawn_worker(),
            cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn snapshot_uncached(&self, pid: u32) -> DialogSnapshot {
        let Some(windows) = ask(
            &self.worker,
            |r| Job::TopLevel { pid, resp: r },
            SNAPSHOT_BUDGET,
        )
        .and_then(|w| w.ok()) else {
            tracing::warn!(pid, "dialog snapshot probe failed/timeout - fail open");
            return DialogSnapshot::default();
        };
        let popups = windows
            .iter()
            .filter(|w| !classify::is_chrome_title(&w.title))
            .filter(|w| !classify::is_system_helper(&w.class))
            .map(classify::popup_from_window)
            .collect::<Vec<_>>();
        let window_status = if !popups.is_empty() {
            Some(tdmcp_core::WindowStatus::BlockedByModalWindow)
        } else if let Some(main) = windows.iter().find(|w| classify::is_main_candidate(w)) {
            let hung = ask(
                &self.worker,
                |r| Job::Hung {
                    id: main.id.clone(),
                    budget_ms: 800,
                    resp: r,
                },
                SNAPSHOT_BUDGET,
            )
            .unwrap_or(false);
            if hung {
                Some(tdmcp_core::WindowStatus::NotResponding)
            } else {
                Some(tdmcp_core::WindowStatus::Responsive)
            }
        } else {
            None
        };
        DialogSnapshot {
            popups,
            window_status,
        }
    }

    fn snapshot_popups_only(&self, pid: u32) -> Vec<PopupInfo> {
        let Some(windows) = ask(
            &self.worker,
            |r| Job::TopLevel { pid, resp: r },
            SNAPSHOT_BUDGET,
        )
        .and_then(|w| w.ok()) else {
            return Vec::new();
        };
        windows
            .iter()
            .filter(|w| !classify::is_chrome_title(&w.title))
            .filter(|w| !classify::is_system_helper(&w.class))
            .map(classify::popup_from_window)
            .collect()
    }

    fn find_popup(&self, pid: u32, id: &str) -> Result<PopupInfo, DialogError> {
        self.snapshot(pid)
            .popups
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| DialogError::NotFound { id: id.to_string() })
    }

    fn children_of(&self, id: &str) -> Vec<crate::sys::SysControl> {
        ask(
            &self.worker,
            |r| Job::Children {
                id: id.to_string(),
                resp: r,
            },
            DESCRIBE_BUDGET,
        )
        .unwrap_or_default()
    }

    /// Verify-gone loop (POC lesson: never fire-and-forget). Errors carry the
    /// still-open id when the window survives the whole ladder.
    fn verify_gone(&self, pid: u32, id: &str, deadline: Instant) -> Result<(), DialogError> {
        while Instant::now() < deadline {
            std::thread::sleep(VERIFY_DELAY);
            if !self.snapshot_popups_only(pid).iter().any(|p| p.id == id) {
                self.cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&pid);
                return Ok(());
            }
        }
        Err(DialogError::DismissFailed { id: id.to_string() })
    }

    /// Safe image-name query used by `kill_td` pid verification.
    pub fn process_image_name(&self, pid: u32) -> Option<String> {
        ask(
            &self.worker,
            |r| Job::ImageName { pid, resp: r },
            DESCRIBE_BUDGET,
        )
        .flatten()
    }
}

impl Default for PlatformDialogSource {
    fn default() -> Self {
        Self::new()
    }
}

impl DialogSource for PlatformDialogSource {
    fn process_image_name(&self, pid: u32) -> Option<String> {
        platform::process_image_name(pid)
    }

    fn process_alive(&self, pid: u32) -> bool {
        platform::process_alive(pid)
    }

    fn snapshot(&self, pid: u32) -> DialogSnapshot {
        {
            let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            if let Some((at, snap)) = cache.get(&pid) {
                if at.elapsed() < CACHE_TTL {
                    return snap.clone();
                }
            }
        }
        let snap = self.snapshot_uncached(pid);
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(pid, (Instant::now(), snap.clone()));
        snap
    }

    fn describe(&self, _pid: u32, id: &str) -> Result<PopupInfo, DialogError> {
        #[cfg(target_os = "macos")]
        if !platform::accessibility_trusted() {
            return Err(DialogError::PermissionDenied);
        }
        let children = self.children_of(id);
        Ok(classify::fill_content(
            classify::popup_from_stub(id),
            &children,
        ))
    }

    fn dismiss(
        &self,
        pid: u32,
        id: &str,
        button: Option<&str>,
    ) -> Result<DismissOutcome, DialogError> {
        #[cfg(target_os = "macos")]
        if !platform::accessibility_trusted() {
            return Err(DialogError::PermissionDenied);
        }
        let deadline = Instant::now() + DISMISS_BUDGET;
        let popup = self.find_popup(pid, id)?;
        if popup.is_main_chrome {
            return Err(DialogError::ChromeProtected { id: id.to_string() });
        }
        let children = self.children_of(id);
        match policy::plan_ladder(button, &children) {
            policy::LadderStep::Click(ctrl_id, label) => {
                let sent = ask(
                    &self.worker,
                    |r| Job::Click {
                        id: id.to_string(),
                        ctrl_id,
                        resp: r,
                    },
                    DESCRIBE_BUDGET,
                )
                .unwrap_or(false);
                if !sent {
                    return Err(DialogError::DismissFailed { id: id.to_string() });
                }
                self.verify_gone(pid, id, deadline)?;
                Ok(DismissOutcome {
                    dismissed: true,
                    via: Some(format!("button:{label}")),
                    still_open: Vec::new(),
                })
            }
            policy::LadderStep::Close => {
                let sent = ask(
                    &self.worker,
                    |r| Job::Close {
                        id: id.to_string(),
                        resp: r,
                    },
                    DESCRIBE_BUDGET,
                )
                .unwrap_or(false);
                if !sent {
                    return Err(DialogError::DismissFailed { id: id.to_string() });
                }
                self.verify_gone(pid, id, deadline)?;
                Ok(DismissOutcome {
                    dismissed: true,
                    via: Some("close".into()),
                    still_open: Vec::new(),
                })
            }
        }
    }
}
