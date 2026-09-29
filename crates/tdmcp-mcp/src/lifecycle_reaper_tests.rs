#![allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
use super::*;
use std::process::{Child, Command, Stdio};

const STOP_ENV: &str = "TDMCP_TEST_REAPER_STOP";

#[test]
#[ignore = "task-owned child fixture, launched only by reaper tests"]
fn owned_child_fixture() {
    let Some(stop) = std::env::var_os(STOP_ENV) else {
        return;
    };
    let stop = Path::new(&stop);
    std::fs::write(stop.parent().unwrap().join("ready"), b"ready").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !stop.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}

async fn fixture() -> (
    tempfile::TempDir,
    Child,
    Arc<AsyncMutex<PidRegistry>>,
    chrono::DateTime<chrono::Utc>,
) {
    let dir = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    let child = Command::new(&exe)
        .args([
            "--exact",
            "lifecycle::reaper_tests::owned_child_fixture",
            "--ignored",
        ])
        .env(STOP_ENV, dir.path().join("stop"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !dir.path().join("ready").is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned test child must enter its controlled loop");
    let reg = Arc::new(AsyncMutex::new(PidRegistry::new()));
    let started = chrono::Utc::now();
    assert!(reg.lock().await.register_starting(
        child.id(),
        SpawnRecord {
            started_at: started,
            exe_path: exe.to_string_lossy().into_owned(),
            expected_project: None,
        }
    ));
    (dir, child, reg, started)
}

async fn finish(dir: &tempfile::TempDir, owner: tokio::task::JoinHandle<()>) {
    std::fs::write(dir.path().join("stop"), b"exit").unwrap();
    tokio::time::timeout(Duration::from_secs(5), owner)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn handshake_reply_does_not_end_child_ownership() {
    let (dir, child, reg, started) = fixture().await;
    let pid = child.id();
    reg.lock()
        .await
        .handshake(pid, tdmcp_core::ProcessAttrs::default(), None);
    let (reply, owner) = spawn_waiter(
        child,
        reg.clone(),
        pid,
        started,
        Duration::from_secs(1),
        None,
    );
    assert_eq!(reply.await.unwrap()["ok"], true);
    assert!(
        !owner.is_finished(),
        "reply must not discard the running child"
    );
    finish(&dir, owner).await;
    assert_eq!(
        reg.lock().await.get(pid).unwrap().bridge,
        tdmcp_core::BridgeStatus::Connected
    );
}

/// Found by a live run, not by a test: this host has TWO TouchDesigner
/// installs — a free build in /opt with no licence and a Pro-activated one
/// inside the Wine prefix — and the spawn reply named neither. An acceptance
/// run drove the wrong product for a whole session without noticing, because
/// nothing in the payload said which build had answered. A reply that cannot
/// name its build cannot be evidence about one.
#[tokio::test]
async fn a_handshake_names_the_build_that_actually_ran() {
    let (dir, child, reg, started) = fixture().await;
    let pid = child.id();
    reg.lock()
        .await
        .handshake(pid, tdmcp_core::ProcessAttrs::default(), None);
    let (reply, owner) = spawn_waiter(
        child,
        reg.clone(),
        pid,
        started,
        Duration::from_secs(1),
        None,
    );
    let value = reply.await.unwrap();

    // The fixture registers THIS test binary as the stand-in exe, so the
    // contract to pin is not the path's contents but that the field carries
    // the exe that actually ran and the requested side stays separately null.
    let resolved = value["spawn"]["resolvedExe"].as_str().unwrap_or_default();
    assert_eq!(
        resolved,
        reg.lock()
            .await
            .get(pid)
            .unwrap()
            .spawn
            .as_ref()
            .unwrap()
            .exe_path,
        "the reply must report the resolved exe verbatim: {value}"
    );
    assert!(!resolved.is_empty(), "the reply must name the exe: {value}");
    // Discovery chose the install, so nothing was requested. That has to stay
    // visible as null rather than being backfilled with the resolved path: a
    // caller comparing the two fields needs to see that no request was made.
    assert_eq!(
        value["spawn"]["requestedExe"],
        Value::Null,
        "an unset exePath is not the same as one that matched: {value}"
    );
    finish(&dir, owner).await;
}

/// The case that actually bit: the configured Wine launcher forwards `--exe`,
/// so a caller-supplied path can pin an exe and a wrapper can run a DIFFERENT
/// build. The resolved path is the honest answer, and the divergence between
/// the two fields has to survive into the reply.
#[tokio::test]
async fn a_requested_exe_that_is_not_the_one_that_ran_stays_visible() {
    let (dir, child, reg, started) = fixture().await;
    let pid = child.id();
    reg.lock()
        .await
        .handshake(pid, tdmcp_core::ProcessAttrs::default(), None);
    let (reply, owner) = spawn_waiter(
        child,
        reg.clone(),
        pid,
        started,
        Duration::from_secs(1),
        Some("/opt/touchdesigner/td/bin/TouchDesigner.exe"),
    );
    let value = reply.await.unwrap();

    assert_eq!(
        value["spawn"]["requestedExe"], "/opt/touchdesigner/td/bin/TouchDesigner.exe",
        "{value}"
    );
    assert!(
        value["spawn"]["resolvedExe"] != value["spawn"]["requestedExe"],
        "the two must be reported independently: a reply that echoes the \
         requested path back is exactly the claim this field exists to falsify: {value}"
    );
    finish(&dir, owner).await;
}

#[tokio::test]
async fn timeout_reply_is_followed_by_reaping_and_starting_cleanup() {
    let (dir, child, reg, started) = fixture().await;
    let pid = child.id();
    let (reply, owner) = spawn_waiter(child, reg.clone(), pid, started, Duration::ZERO, None);
    let value = reply.await.unwrap();
    assert_eq!(value["outcome"], "wait_timeout");
    assert_eq!(value["stillAlive"], true);
    assert!(!owner.is_finished());
    finish(&dir, owner).await;
    assert!(reg.lock().await.get(pid).is_none());
}

/// Found by a live run, not by the tests above: the timeout payload used to be
/// `{ok:false, outcome, pid, stillAlive}` and nothing else. A caller respecting
/// it had no way to learn that the process was STILL RUNNING and unhandshaken,
/// so the only thing preventing a second TouchDesigner was a skill card — a
/// safety property living in documentation. The state a caller acts on has to
/// be in the payload itself.
#[tokio::test]
async fn a_timeout_reply_carries_the_state_a_caller_acts_on() {
    let (dir, child, reg, started) = fixture().await;
    let pid = child.id();
    let (reply, owner) = spawn_waiter(child, reg.clone(), pid, started, Duration::ZERO, None);
    let value = reply.await.unwrap();

    assert_eq!(
        value["code"],
        tdmcp_diagnostics::codes::SPAWN_WAIT_TIMEOUT,
        "a non-handshake outcome still needs its catalog code: {value}"
    );
    assert_eq!(
        value["effect"], "unknown",
        "the launch happened and its result is not yet knowable: {value}"
    );
    assert_eq!(value["pid"], pid, "{value}");
    assert_eq!(value["stillAlive"], true, "{value}");
    let guidance = value["guidance"].as_str().unwrap_or_default();
    assert!(
        guidance.contains("Do NOT call spawn_td again"),
        "the anti-double-spawn guard must be in the payload a caller reads, not only in a card: {guidance}"
    );
    // The still-alive case is the one that matters, so it is asserted
    // separately from the generic text: a payload that only warns "check fleet"
    // is what let a second TouchDesigner start.
    assert!(
        guidance.contains("still running") || guidance.contains("stillAlive"),
        "the payload must connect the timeout to the process's real state: {guidance}"
    );
    finish(&dir, owner).await;
}

/// A vanished registry row is not a process exit. It took the same bare
/// payload as the deadline case, and it needs the same honesty, plus the fact
/// that the row itself is what went missing.
#[tokio::test]
async fn a_vanished_registry_row_says_so_rather_than_blaming_the_budget() {
    let (dir, child, reg, started) = fixture().await;
    let pid = child.id();
    reg.lock().await.remove_starting(pid);
    let (reply, owner) = spawn_waiter(child, reg, pid, started, Duration::ZERO, None);
    let value = reply.await.unwrap();

    assert_eq!(value["outcome"], "wait_timeout");
    assert_eq!(value["registryRowPresent"], false, "{value}");
    assert_eq!(value["effect"], "unknown", "{value}");
    let guidance = value["guidance"].as_str().unwrap_or_default();
    assert!(
        guidance.contains("not proof the process died"),
        "losing the row is not evidence of death and must not read as it: {guidance}"
    );
    finish(&dir, owner).await;
}

#[tokio::test]
async fn disconnected_client_does_not_cancel_owned_reaper() {
    let (dir, child, reg, started) = fixture().await;
    let pid = child.id();
    let (reply, owner) = spawn_waiter(child, reg.clone(), pid, started, Duration::ZERO, None);
    drop(reply);
    finish(&dir, owner).await;
    assert!(reg.lock().await.get(pid).is_none());
}

#[tokio::test]
async fn vanished_registry_row_does_not_end_child_ownership() {
    let (dir, child, reg, started) = fixture().await;
    let pid = child.id();
    reg.lock().await.remove_starting(pid);
    let (reply, owner) = spawn_waiter(child, reg, pid, started, Duration::ZERO, None);
    assert_eq!(reply.await.unwrap()["outcome"], "wait_timeout");
    assert!(!owner.is_finished());
    finish(&dir, owner).await;
}

#[tokio::test]
async fn replacement_spawn_provenance_is_never_removed_or_reported_as_ours() {
    let (dir, child, reg, started) = fixture().await;
    let pid = child.id();
    let replacement = started + chrono::Duration::seconds(1);
    reg.lock()
        .await
        .get_mut(pid)
        .unwrap()
        .spawn
        .as_mut()
        .unwrap()
        .started_at = replacement;
    let (reply, owner) = spawn_waiter(child, reg.clone(), pid, started, Duration::ZERO, None);
    assert_eq!(reply.await.unwrap()["outcome"], "wait_timeout");
    finish(&dir, owner).await;
    assert_eq!(
        reg.lock()
            .await
            .get(pid)
            .unwrap()
            .spawn
            .as_ref()
            .unwrap()
            .started_at,
        replacement
    );
}

#[tokio::test]
async fn late_handshake_remains_ipc_owned_after_wait_timeout_and_exit() {
    let (dir, child, reg, started) = fixture().await;
    let pid = child.id();
    let (reply, owner) = spawn_waiter(child, reg.clone(), pid, started, Duration::ZERO, None);
    assert_eq!(reply.await.unwrap()["outcome"], "wait_timeout");
    reg.lock()
        .await
        .handshake(pid, tdmcp_core::ProcessAttrs::default(), None);
    finish(&dir, owner).await;
    assert_eq!(
        reg.lock().await.get(pid).unwrap().bridge,
        tdmcp_core::BridgeStatus::Connected
    );
}
