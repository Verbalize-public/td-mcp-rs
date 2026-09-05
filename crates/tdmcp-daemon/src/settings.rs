//! Serialized, validated settings writes and notification of live consumers.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tdmcp_config::ConfigFile;
use tokio::sync::{watch, Mutex};

/// Shared by the admin endpoint and runtime configuration consumers.
#[derive(Clone)]
pub struct Settings {
    path: PathBuf,
    startup: Arc<ConfigFile>,
    updates: watch::Sender<ConfigFile>,
    writer: Arc<Mutex<()>>,
}

impl Settings {
    /// Initialize from the file snapshot actually used at startup.
    pub fn new(path: PathBuf, config: ConfigFile) -> Self {
        let (updates, _) = watch::channel(config.clone());
        Self {
            path,
            startup: Arc::new(config),
            updates,
            writer: Arc::new(Mutex::new(())),
        }
    }

    /// Most recently applied settings (listener and process settings still need restart).
    pub fn current(&self) -> ConfigFile {
        self.updates.borrow().clone()
    }

    /// Original file settings, before any edits.
    pub fn startup(&self) -> &ConfigFile {
        &self.startup
    }

    /// Subscribe to successful updates.
    pub fn subscribe(&self) -> watch::Receiver<ConfigFile> {
        self.updates.subscribe()
    }

    /// Settings saved since startup which still need a restart.
    pub fn restart_required(&self) -> Vec<&'static str> {
        tdmcp_config::restart_required_fields(&self.startup, &self.current())
    }

    /// Merge and persist before notifying consumers. Disk I/O runs off the runtime.
    pub async fn patch(&self, patch: Value) -> anyhow::Result<ConfigFile> {
        let _writer = self.writer.lock().await;
        let path = self.path.clone();
        let updated = tokio::task::spawn_blocking(move || {
            let current = tdmcp_config::load(&path)?;
            let updated = tdmcp_config::merge_patch(&current, &patch)?;
            anyhow::ensure!(
                updated.federation.daemon_id == current.federation.daemon_id,
                "federation.daemon_id cannot be changed through settings"
            );
            tdmcp_config::save(&path, &updated)?;
            Ok::<_, anyhow::Error>(updated)
        })
        .await??;
        tdmcp_mcp::init_bridge_timeouts(
            updated
                .bridge
                .call_timeout_secs
                .max(updated.bridge.script_timeout_secs),
        );
        self.updates.send_replace(updated.clone());
        Ok(updated)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test assertions")]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (tempfile::TempDir, Settings) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let mut cfg = ConfigFile::default();
        cfg.federation.daemon_id = "stable-test-id".into();
        tdmcp_config::save(&path, &cfg).expect("save");
        (dir, Settings::new(path, cfg))
    }

    #[tokio::test]
    async fn concurrent_disjoint_edits_are_preserved() {
        let (_dir, settings) = fixture();
        let (a, b) = tokio::join!(
            settings.patch(json!({"bridge":{"callTimeoutSecs":77}})),
            settings.patch(json!({"daemon":{"keepAlive":false}})),
        );
        a.expect("first save");
        b.expect("second save");
        assert_eq!(settings.current().bridge.call_timeout_secs, 77);
        assert!(!settings.current().daemon.keep_alive);
        assert!(settings.restart_required().is_empty());
    }

    #[tokio::test]
    async fn reverting_saved_listener_clears_pending_restart() {
        let (_dir, settings) = fixture();
        let port = settings.current().server.port;
        settings
            .patch(json!({"server":{"port":port + 1}}))
            .await
            .expect("save");
        assert!(!settings.restart_required().is_empty());
        settings
            .patch(json!({"server":{"port":port}}))
            .await
            .expect("revert");
        assert!(settings.restart_required().is_empty());
    }

    #[tokio::test]
    async fn rejected_save_changes_neither_disk_nor_watch_state() {
        let (_dir, settings) = fixture();
        let mut updates = settings.subscribe();
        let before = std::fs::read(&settings.path).expect("read");
        for patch in [
            json!({"server":{"port":0}}),
            json!({"federation":{"daemonId":"replacement"}}),
        ] {
            assert!(settings.patch(patch).await.is_err());
        }
        assert_eq!(std::fs::read(&settings.path).expect("read"), before);
        assert!(!updates.has_changed().expect("watch"));
        assert_eq!(
            updates.borrow_and_update().federation.daemon_id,
            "stable-test-id"
        );
    }
}
