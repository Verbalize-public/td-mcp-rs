//! Bridge handshake over the real TCP socket path (A4 of the Linux-support
//! spec): a fake bridge peer completes the handshake against a bound
//! [`IpcListener`]; a port-scanner-style garbage frame and an unsupported
//! `protocol_version` each get one framed JSON error, then a close.
//!
//! The listener binds port 0 (OS-assigned) so the tests are parallel-safe.

#![allow(clippy::unwrap_used, reason = "test setup/assertions may panic")]
#![allow(clippy::expect_used, reason = "test setup/assertions may panic")]
#![allow(clippy::panic, reason = "test assertions may panic")]

use tdmcp_ipc::{
    encode, BridgeEndpoint, HandshakeRequest, IpcError, IpcListener, Message,
    HANDSHAKE_INVALID_CODE, HANDSHAKE_IO_TIMEOUT, PROTOCOL_MISMATCH_CODE, PROTOCOL_VERSION,
};
use tdmcp_test_support::FakeTdPeer;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Bind a real TCP bridge listener on an ephemeral port.
async fn bind_listener() -> IpcListener {
    IpcListener::bind(BridgeEndpoint::Tcp {
        host: "127.0.0.1".into(),
        port: 0,
    })
    .await
    .expect("bind bridge listener")
}

/// Drive one `accept_handshake` on the spawned side.
fn accept(
    listener: IpcListener,
) -> tokio::task::JoinHandle<Result<tdmcp_ipc::IpcStream, IpcError>> {
    tokio::spawn(async move {
        listener
            .accept_handshake("/bridge", "9.9.9-test", Default::default())
            .await
    })
}

/// `expect_err` needs `T: Debug`; `IpcStream` has none by design.
async fn expect_reject(
    task: tokio::task::JoinHandle<Result<tdmcp_ipc::IpcStream, IpcError>>,
) -> IpcError {
    match task.await.expect("accept task") {
        Ok(_) => panic!("handshake must be rejected"),
        Err(e) => e,
    }
}

/// Read one framed JSON value from the peer side, then assert the daemon
/// closed the connection (EOF).
async fn read_error_then_eof(client: &mut TcpStream) -> serde_json::Value {
    let mut len_buf = [0u8; 4];
    client
        .read_exact(&mut len_buf)
        .await
        .expect("error frame length");
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut body = vec![0u8; len];
    client.read_exact(&mut body).await.expect("error frame body");
    let value: serde_json::Value =
        serde_json::from_slice(&body).expect("error frame is valid JSON");
    let mut eof = [0u8; 1];
    assert_eq!(
        client.read(&mut eof).await.expect("eof read"),
        0,
        "daemon must close the connection after the error frame"
    );
    value
}

#[tokio::test]
async fn fake_peer_completes_handshake_over_real_tcp() {
    let listener = bind_listener().await;
    let port = listener.local_addr().port();
    let server = accept(listener);

    let mut peer = FakeTdPeer::connect(("127.0.0.1", port), 4242)
        .await
        .expect("dial bridge listener");
    let resp = peer.handshake("fake.toe").await.expect("handshake");
    assert_eq!(resp.bridge_package_dir, "/bridge");
    assert_eq!(resp.daemon_version, "9.9.9-test");

    // Handshake stream carries requests both ways: daemon pings, peer answers.
    let mut stream = server.await.expect("accept ok").expect("handshake accepted");
    assert_eq!(stream.pid, 4242);
    assert_eq!(stream.handshake.protocol_version, PROTOCOL_VERSION);
    stream
        .send(&Message::Request {
            id: "p1".into(),
            method: "ping".into(),
            params: serde_json::json!({}),
        })
        .await
        .expect("send ping");
    let ping = peer.recv_message().await.expect("recv ping");
    let Message::Request { id, method, .. } = ping else {
        panic!("expected request, got {ping:?}");
    };
    assert_eq!(method, "ping");
    peer.send_response(id, serde_json::json!({"ok": true, "pong": true}))
        .await
        .expect("send pong");
    let pong = stream.recv_message().await.expect("recv pong");
    let Message::Response { id, result, .. } = pong else {
        panic!("expected response, got {pong:?}");
    };
    assert_eq!(id, "p1");
    assert_eq!(result.unwrap()["pong"], true);
}

#[tokio::test]
async fn garbage_first_frame_gets_framed_error_then_disconnect() {
    let listener = bind_listener().await;
    let port = listener.local_addr().port();
    let server = accept(listener);

    let mut client = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("dial bridge listener");
    client
        .write_all(b"GET / HTTP/1.1 rubbish-not-a-frame")
        .await
        .expect("write garbage");
    // Half-close so the daemon's drain ends immediately (no 1s drain budget).
    client.shutdown().await.expect("half-close");
    let value = read_error_then_eof(&mut client).await;
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], HANDSHAKE_INVALID_CODE);
    assert!(
        value["message"].as_str().unwrap().contains("re-embed"),
        "rejection must carry the re-embed hint: {value}"
    );

    // Daemon side: the handshake is rejected, the accept loop stays alive.
    let err = expect_reject(server).await;
    assert!(matches!(err, IpcError::Frame(_)), "got {err}");
}

#[tokio::test]
async fn protocol_version_999_gets_mismatch_error_then_disconnect() {
    let listener = bind_listener().await;
    let port = listener.local_addr().port();
    let server = accept(listener);

    let mut client = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("dial bridge listener");
    let req = HandshakeRequest {
        pid: 4242,
        protocol_version: "999".into(),
        title: Some("old.toe".into()),
        toe_path: None,
        image: None,
        start_time: None,
    };
    client
        .write_all(&encode(&req).expect("encode handshake"))
        .await
        .expect("write handshake");
    // Half-close so the daemon's drain ends immediately (no 1s drain budget).
    client.shutdown().await.expect("half-close");
    let value = read_error_then_eof(&mut client).await;
    assert_eq!(value["ok"], false);
    assert_eq!(value["code"], PROTOCOL_MISMATCH_CODE);
    assert!(
        value["message"].as_str().unwrap().contains("re-embed"),
        "rejection must carry the re-embed hint: {value}"
    );

    let err = expect_reject(server).await;
    assert!(matches!(err, IpcError::Handshake(_)), "got {err}");
}

#[tokio::test]
async fn silent_peer_times_out_within_budget_and_listener_survives() {
    let listener = bind_listener().await;
    let port = listener.local_addr().port();
    let server = accept(listener);

    // Connect and say nothing — the classic TCP health probe.
    let _silent = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("dial bridge listener");
    let started = std::time::Instant::now();
    let err = expect_reject(server).await;
    assert!(
        matches!(err, IpcError::HandshakeTimeout(_)),
        "got {err}"
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed >= HANDSHAKE_IO_TIMEOUT && elapsed < HANDSHAKE_IO_TIMEOUT + std::time::Duration::from_secs(3),
        "timeout must respect the budget, took {elapsed:?}"
    );
}
