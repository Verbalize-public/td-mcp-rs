//! Daemon-side popup watcher (DIALOGS.md §5.3).
//!
//! Every `[dialogs].poll_ms`, sample registered pids whose bridge is
//! `{Starting, Connected}` through the platform [`DialogSource`], refresh the
//! shared snapshot map, and fill each entry's reserved `window_status`.
//!
//! Idle-liveness inert: touches nothing the idle clock counts.

use std::sync::Arc;

use tdmcp_core::{BridgeStatus, DialogSnapshot, DialogSource};
use tokio::sync::Mutex as AsyncMutex;

/// Shared state installed into `tdmcp_mcp::dialogs` by the daemon.
pub type Shared = tdmcp_mcp::dialogs::DialogsShared;

/// Build the platform source: Win32 on Windows, Null elsewhere.
#[must_use]
pub fn build_source() -> Arc<dyn DialogSource> {
    #[cfg(windows)]
    {
        Arc::new(tdmcp_dialogs::Win32Source::new())
    }
    #[cfg(not(windows))]
    {
        Arc::new(tdmcp_core::NullDialogSource)
    }
}

/// One sweep — samples eligible pids, writes window_status back into the
/// registry, refreshes the shared snapshot map, purges stale rows.
///
/// Platform probes are blocking user32/UIA calls executed inline; with the
/// default 1 s cadence and a handful of pids this stays well off the async
/// workers' critical path.
pub async fn sweep_once(registry: &AsyncMutex<tdmcp_core::PidRegistry>, shared: &Shared) -> usize {
    // Short lock #1: pick pids + purge stale cache rows.
    let pids: Vec<u32> = {
        let reg = registry.lock().await;
        let live = reg.pids();
        let filtered: Vec<u32> = live
            .iter()
            .copied()
            .filter(|pid| {
                matches!(
                    reg.get(*pid).map(|e| e.bridge),
                    Some(BridgeStatus::Connected) | Some(BridgeStatus::Starting)
                )
            })
            .collect();
        {
            let mut map = shared.snapshots.lock().unwrap_or_else(|p| p.into_inner());
            map.retain(|k, _| filtered.contains(k));
        }
        filtered
    };
    for pid in &pids {
        let snap: DialogSnapshot = shared.source.snapshot(*pid);
        if let Some(status) = snap.window_status {
            let mut reg = registry.lock().await;
            if let Some(entry) = reg.get_mut(*pid) {
                entry.process.window_status = Some(status.as_str().to_string());
            }
        }
        shared
            .snapshots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(*pid, snap);
    }
    pids.len()
}

/// Watcher loop: sweep every tick until shutdown.
pub async fn run_dialogs_watcher(
    registry: Arc<AsyncMutex<tdmcp_core::PidRegistry>>,
    shared: Arc<Shared>,
    poll_ms: u64,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(poll_ms.max(100)));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = ticker.tick() => {
                sweep_once(&registry, &shared).await;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use tdmcp_core::{
        DialogSeverity, PopupInfo, ProcessAttrs, ProcessFingerprint, SpawnRecord, WindowStatus,
    };

    struct FakeSource;

    impl DialogSource for FakeSource {
        fn snapshot(&self, pid: u32) -> DialogSnapshot {
            if pid == 7 {
                DialogSnapshot {
                    popups: vec![PopupInfo {
                        id: "42".into(),
                        title: "Backwards Compatiblity Issue".into(),
                        class: None,
                        kind: tdmcp_core::PopupKind::Unknown,
                        severity: DialogSeverity::Soft,
                        message: None,
                        buttons: Vec::new(),
                        is_main_chrome: false,
                    }],
                    window_status: Some(WindowStatus::BlockedByModalWindow),
                }
            } else {
                DialogSnapshot {
                    popups: Vec::new(),
                    window_status: Some(WindowStatus::Responsive),
                }
            }
        }

        fn describe(
            &self,
            _pid: u32,
            _id: &str,
        ) -> Result<tdmcp_core::PopupInfo, tdmcp_core::DialogError> {
            Err(tdmcp_core::DialogError::Unsupported)
        }

        fn dismiss(
            &self,
            _pid: u32,
            _id: &str,
            _button: Option<&str>,
        ) -> Result<tdmcp_core::DismissOutcome, tdmcp_core::DialogError> {
            Err(tdmcp_core::DialogError::Unsupported)
        }
    }

    fn attrs(title: &str) -> ProcessAttrs {
        ProcessAttrs {
            title: Some(title.into()),
            fingerprint: ProcessFingerprint {
                title: Some(title.into()),
                image: Some("TouchDesigner.exe".into()),
                start_time: Some("t0".into()),
            },
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn sweep_writes_window_status_and_snapshots_for_eligible_pids() {
        use std::collections::HashMap;
        let reg = AsyncMutex::new(PidRegistryShim::registry());
        let shared = Shared {
            source: Arc::new(FakeSource),
            snapshots: std::sync::Mutex::new(HashMap::new()),
        };
        let n = sweep_once(&reg, &shared).await;
        assert_eq!(n, 2, "starting + connected sampled");
        let (p7, w8) = {
            let map = shared.snapshots.lock().unwrap();
            (map[&7].popups.len(), map[&8].window_status)
        };
        assert_eq!(p7, 1);
        assert_eq!(w8, Some(WindowStatus::Responsive));
        let reg = reg.lock().await;
        assert_eq!(
            reg.get(7).unwrap().process.window_status.as_deref(),
            Some("blocked_by_modal_window")
        );
        assert_eq!(
            reg.get(9).unwrap().process.window_status,
            None,
            "disconnected pid not sampled"
        );
    }

    /// Tiny helper so the test owns a registry in one expression.
    struct PidRegistryShim;
    impl PidRegistryShim {
        fn registry() -> tdmcp_core::PidRegistry {
            use tdmcp_core::PidRegistry;
            let mut r = PidRegistry::new();
            r.register_starting(
                7,
                SpawnRecord {
                    started_at: chrono::Utc::now(),
                    exe_path: "e".into(),
                    expected_project: None,
                },
            );
            r.handshake(8, attrs("b"), Some("1".into()));
            r.handshake(9, attrs("c"), Some("1".into()));
            r.on_bridge_lost(9, chrono::Utc::now()); // disconnected → not sampled
            r
        }
    }
}
