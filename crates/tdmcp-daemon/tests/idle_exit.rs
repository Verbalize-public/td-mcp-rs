//! Binary smoke: empty daemon exits after idle timeout.
//!
//! Uses a Drop harness so a failed assertion cannot leave a zombie daemon.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tdmcp_daemon::{health_ok, read_daemon_lock_pid};
use tempfile::tempdir;

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local_addr").port()
}

fn daemon_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tdmcp-daemon"))
}

fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
}

struct IdleHarness {
    port: u16,
    data_dir: PathBuf,
    child: Option<Child>,
}

impl IdleHarness {
    fn spawn(port: u16, data_dir: &Path, idle_secs: u64) -> Self {
        let pipe = format!(r"\\.\pipe\tdmcp-rs-idle-test-{port}");
        let config_path = data_dir.join("test-config.toml");
        tdmcp_config::ensure_default(&config_path, true).expect("seed config");
        let child = Command::new(daemon_bin())
            .args([
                "start",
                "--port",
                &port.to_string(),
                "--data-dir",
                data_dir.to_str().expect("utf8 data_dir"),
                "--no-gui",
            ])
            .env("TDMCP_IDLE_EXIT_SECS", idle_secs.to_string())
            .env("TDMCP_IPC_PIPE", pipe)
            .env(tdmcp_config::CONFIG_PATH_ENV, &config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn start");
        Self {
            port,
            data_dir: data_dir.to_path_buf(),
            child: Some(child),
        }
    }

    async fn wait_exit(&mut self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            match self.child.as_mut().expect("child").try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) => tokio::time::sleep(Duration::from_millis(50)).await,
                Err(_) => return true,
            }
        }
        false
    }

    fn force_kill(&mut self) {
        if let Some(pid) = read_daemon_lock_pid(&self.data_dir) {
            kill_pid(pid);
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(self.data_dir.join("daemon.lock"));
    }
}

impl Drop for IdleHarness {
    fn drop(&mut self) {
        let _ = Command::new(daemon_bin())
            .args(["stop", "--port", &self.port.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        std::thread::sleep(Duration::from_millis(300));
        self.force_kill();
    }
}

async fn wait_healthy(port: u16, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if health_ok(port).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("daemon not healthy on port {port} within {timeout:?}");
}

#[tokio::test]
async fn empty_daemon_exits_after_idle_timeout() {
    let dir = tempdir().expect("tempdir");
    let data_dir = dir.path();
    let port = free_port();
    let mut harness = IdleHarness::spawn(port, data_dir, 2);

    wait_healthy(port, Duration::from_secs(10)).await;

    assert!(
        harness.wait_exit(Duration::from_secs(10)).await,
        "daemon child must exit after idle timeout without taskkill"
    );
    assert!(
        !health_ok(port).await,
        "port must be unhealthy after idle exit"
    );

    let lock = data_dir.join("daemon.lock");
    let lock_deadline = std::time::Instant::now() + Duration::from_secs(2);
    while lock.exists() && std::time::Instant::now() < lock_deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !lock.exists(),
        "daemon.lock should be removed after idle exit"
    );

    // Clean exit — don't force-kill in Drop.
    let _ = harness.child.take();
}
