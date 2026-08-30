//! Local IPC framing, handshake, and TCP-loopback transport for td-mcp-rs.

#![warn(missing_docs)]

mod framing;
mod handshake;
mod listener;

pub use framing::{encode, try_decode, FrameError, Message};
pub use handshake::{HandshakeOffer, HandshakeRequest, HandshakeResponse};
pub use listener::{
    BridgeEndpoint, IpcError, IpcListener, IpcStream, HANDSHAKE_IO_TIMEOUT,
    HANDSHAKE_INVALID_CODE, HANDSHAKE_TIMEOUT_CODE, PROTOCOL_MISMATCH_CODE, PROTOCOL_VERSION,
};
