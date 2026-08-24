//! Fake TD bridge peer for integration tests.

#![warn(missing_docs)]

use tdmcp_ipc::{encode, FrameError, HandshakeRequest, HandshakeResponse, IpcError, Message};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

/// A fake TD peer that speaks the real IPC handshake + message protocol.
pub struct FakeTdPeer {
    stream: DuplexStream,
    /// Pid announced at handshake.
    pub pid: u32,
}

impl FakeTdPeer {
    /// Create a connected pair: `(fake_peer, daemon_side_stream)`.
    ///
    /// The fake peer has **not** yet performed handshake — call
    /// [`handshake`](Self::handshake) first.
    #[must_use]
    pub fn pair(pid: u32) -> (Self, DuplexStream) {
        let (client, server) = tokio::io::duplex(64 * 1024);
        (
            Self {
                stream: client,
                pid,
            },
            server,
        )
    }

    /// Send handshake and read the daemon response.
    pub async fn handshake(
        &mut self,
        title: impl Into<String>,
    ) -> Result<HandshakeResponse, IpcError> {
        self.handshake_with(title, None).await
    }

    /// Send handshake with optional toe path (project identity).
    pub async fn handshake_with(
        &mut self,
        title: impl Into<String>,
        toe_path: Option<String>,
    ) -> Result<HandshakeResponse, IpcError> {
        let req = HandshakeRequest {
            pid: self.pid,
            protocol_version: "1".into(),
            title: Some(title.into()),
            toe_path,
            image: Some("TouchDesigner.exe".into()),
            start_time: Some("t0".into()),
        };
        write_msg(&mut self.stream, &req).await?;
        read_msg(&mut self.stream).await
    }

    /// Send a response to a prior request.
    pub async fn send_response(
        &mut self,
        id: impl Into<String>,
        result: serde_json::Value,
    ) -> Result<(), IpcError> {
        let msg = Message::Response {
            id: id.into(),
            result: Some(result),
            error: None,
        };
        write_msg(&mut self.stream, &msg).await
    }

    /// Read next framed message.
    pub async fn recv_message(&mut self) -> Result<Message, IpcError> {
        read_msg(&mut self.stream).await
    }

    /// Send a `Message::Event` frame (e.g. the M2 bridge log uplink).
    pub async fn send_event(
        &mut self,
        name: impl Into<String>,
        payload: serde_json::Value,
    ) -> Result<(), IpcError> {
        let msg = Message::Event {
            name: name.into(),
            payload,
        };
        write_msg(&mut self.stream, &msg).await
    }

    /// Answer `ping` (and optionally other methods) until the peer drops.
    ///
    /// Spawns a background task; returns a [`JoinHandle`](tokio::task::JoinHandle).
    pub fn spawn_auto_pong(mut self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let msg = match self.recv_message().await {
                    Ok(m) => m,
                    Err(_) => break,
                };
                let Message::Request { id, method, .. } = msg else {
                    continue;
                };
                let result = match method.as_str() {
                    "ping" => serde_json::json!({"ok": true, "pong": true}),
                    "execute_python" => serde_json::json!({"ok": true, "result": 1}),
                    "capture" => {
                        serde_json::json!({
                            "ok": true,
                            "bytes": 1024,
                            "path": "/project1/out1",
                            "mimeType": "image/png",
                            // Minimal valid-looking base64 stub (not a real PNG).
                            "imageBase64": "iVBORw0KGgo=",
                        })
                    }
                    "inspect" => {
                        serde_json::json!({"ok": true, "nodes": [{"ok": true, "path": "/project1"}]})
                    }
                    _ => serde_json::json!({"ok": true}),
                };
                if self.send_response(id, result).await.is_err() {
                    break;
                }
            }
        })
    }
}

async fn read_msg<T: serde::de::DeserializeOwned>(r: &mut DuplexStream) -> Result<T, IpcError> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body).map_err(FrameError::Json)?)
}

async fn write_msg<T: serde::Serialize>(w: &mut DuplexStream, msg: &T) -> Result<(), IpcError> {
    let bytes = encode(msg)?;
    w.write_all(&bytes).await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use tdmcp_ipc::IpcStream;

    #[tokio::test]
    async fn memory_handshake_roundtrip() {
        let (mut peer, server) = FakeTdPeer::pair(42);
        let server_task = tokio::spawn(async move {
            IpcStream::accept_memory_handshake(server, "/tmp/bridge", "0.1.0")
                .await
                .unwrap()
        });
        let resp = peer
            .handshake_with("demo.toe", Some("/data/demo.toe".into()))
            .await
            .unwrap();
        assert_eq!(resp.bridge_package_dir, "/tmp/bridge");
        let stream = server_task.await.unwrap();
        assert_eq!(stream.pid, 42);
        assert_eq!(stream.handshake.title.as_deref(), Some("demo.toe"));
        assert_eq!(stream.handshake.toe_path.as_deref(), Some("/data/demo.toe"));
    }
}
