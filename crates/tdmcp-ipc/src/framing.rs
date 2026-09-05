//! Length-prefixed JSON frames over a byte stream.

use bytes::{Buf, BytesMut};
use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

/// Framing / codec errors.
#[derive(Debug, Error)]
pub enum FrameError {
    /// Incomplete frame in the buffer.
    #[error("incomplete frame")]
    Incomplete,
    /// Frame larger than the allowed max.
    #[error("frame too large: {0} bytes")]
    TooLarge(usize),
    /// JSON encode/decode failure.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Hard cap on a single framed IPC payload (body bytes). Kept at 2x the
/// client-facing HTTP wire cap (`WIRE_BODY_LIMIT_BYTES`, 16 MiB in
/// `tdmcp-daemon/src/main.rs`) so this internal daemon<->bridge hop never
/// becomes the tighter bottleneck.
pub(crate) const MAX_FRAME: usize = 32 * 1024 * 1024;

/// Encode a serializable message as `u32 LE length + utf8 json`.
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(msg)?;
    if body.len() > MAX_FRAME {
        return Err(FrameError::TooLarge(body.len()));
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Try to decode one frame from `buf`. Advances the buffer on success.
pub fn try_decode<T: DeserializeOwned>(buf: &mut BytesMut) -> Result<Option<T>, FrameError> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let mut len_bytes = [0u8; 4];
    len_bytes.copy_from_slice(&buf[..4]);
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_FRAME {
        return Err(FrameError::TooLarge(len));
    }
    if buf.len() < 4 + len {
        return Ok(None);
    }
    buf.advance(4);
    let body = buf.split_to(len);
    let msg = serde_json::from_slice(&body)?;
    Ok(Some(msg))
}

/// Envelope for all IPC messages after handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// Request from daemon to bridge.
    Request {
        /// Correlation id.
        id: String,
        /// Method name.
        method: String,
        /// JSON params.
        params: serde_json::Value,
    },
    /// Response from bridge to daemon.
    Response {
        /// Correlation id.
        id: String,
        /// Result (mutually exclusive with error).
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
        /// Error object.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<serde_json::Value>,
    },
    /// Bridge → daemon event (optional).
    Event {
        /// Event name.
        name: String,
        /// Payload.
        payload: serde_json::Value,
    },
}

use serde::Deserialize;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let msg = Message::Request {
            id: "1".into(),
            method: "ping".into(),
            params: serde_json::json!({}),
        };
        let bytes = encode(&msg).unwrap();
        let mut buf = BytesMut::from(&bytes[..]);
        let decoded: Message = try_decode(&mut buf).unwrap().unwrap();
        match decoded {
            Message::Request { method, .. } => assert_eq!(method, "ping"),
            other => unreachable!("wrong variant: {other:?}"),
        }
        assert!(buf.is_empty());
    }

    #[test]
    fn oversized_header_is_rejected_without_a_body() {
        let size = MAX_FRAME + 1;
        let header = (size as u32).to_le_bytes();
        let mut buf = BytesMut::from(header.as_slice());
        assert!(matches!(
            try_decode::<Message>(&mut buf),
            Err(FrameError::TooLarge(n)) if n == size
        ));
        assert_eq!(buf.as_ref(), header);
    }

    #[test]
    fn maximum_header_waits_for_body_without_consuming_it() {
        let header = (MAX_FRAME as u32).to_le_bytes();
        let mut buf = BytesMut::from(header.as_slice());
        assert!(try_decode::<Message>(&mut buf).unwrap().is_none());
        assert_eq!(buf.as_ref(), header);
    }
}
