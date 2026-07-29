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
        let req = HandshakeRequest {
            pid: self.pid,
            protocol_version: "1".into(),
            title: Some(title.into()),
            toe_path: None,
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
        let resp = peer.handshake("test").await.unwrap();
        assert_eq!(resp.bridge_package_dir, "/tmp/bridge");
        let stream = server_task.await.unwrap();
        assert_eq!(stream.pid, 42);
    }
}
