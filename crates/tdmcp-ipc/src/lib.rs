//! Local IPC framing and handshake for td-mcp-rs.

#![warn(missing_docs)]

mod framing;
mod handshake;
mod listener;

pub use framing::{encode, try_decode, FrameError, Message};
pub use handshake::{HandshakeRequest, HandshakeResponse};
pub use listener::{BridgeEndpoint, IpcError, IpcListener, IpcStream};
