//! Local TCP-loopback listener for the daemon↔bridge transport.
//!
//! One standard transport on every OS (spec LINUX_SUPPORT D1): the daemon
//! binds a loopback TCP endpoint and each accepted connection performs exactly
//! one handshake under [`HANDSHAKE_IO_TIMEOUT`]. Framing is unchanged
//! (`u32` LE length + UTF-8 JSON).

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::timeout;
use tracing::{info, warn};

use crate::framing::{self, FrameError, Message, MAX_FRAME};
use crate::handshake::{HandshakeOffer, HandshakeRequest, HandshakeResponse};

/// Budget for post-connect handshake frame read + response write.
pub const HANDSHAKE_IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Only wire protocol version the daemon accepts (T-6); the shipped bridge
/// sends the same value from `bridge/tdmcp_bridge/constants.py`.
pub const PROTOCOL_VERSION: &str = "1";

/// Coded error for a bridge whose `protocol_version` predates TCP support
/// (D5: the daemon log and the framed error both carry it).
pub const PROTOCOL_MISMATCH_CODE: &str = "tdmcp.bridge.protocol_mismatch";

/// Coded error for a first frame that is missing/garbled (port scanners,
/// stray HTTP requests).
pub const HANDSHAKE_INVALID_CODE: &str = "tdmcp.bridge.handshake_invalid";

/// Coded error for a peer that never delivers a handshake frame in time.
pub const HANDSHAKE_TIMEOUT_CODE: &str = "tdmcp.bridge.handshake_timeout";

/// Hard cap for the best-effort rejection-frame write: a stuck peer must not
/// stall the accept loop beyond a fraction of the handshake budget.
const ERROR_FRAME_WRITE_BUDGET: Duration = Duration::from_secs(1);

/// IPC endpoint errors.
#[derive(Debug, Error)]
pub enum IpcError {
    /// I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Framing failure.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// Handshake rejected.
    #[error("handshake rejected: {0}")]
    Handshake(String),
    /// Peer connected but did not complete handshake frames in time.
    #[error("handshake I/O timed out after {0:?}")]
    HandshakeTimeout(Duration),
    /// Bind could not claim the address (port taken); the error names
    /// host:port so operators can free it or override `[bridge] port` (T-3).
    #[error("bind failed on {addr}: {source}")]
    Bind {
        /// Address the listener tried to claim.
        addr: SocketAddr,
        /// Underlying OS error.
        source: std::io::Error,
    },
    /// Configured host is not loopback — the bridge port must stay local in
    /// v0 (D2/T-4, same trust model as the MCP HTTP listener without PSK).
    #[error("bridge host {0:?} is not loopback — the bridge port binds 127.0.0.1/::1 only in v0")]
    NotLoopback(String),
}

/// Resolved endpoint for the bridge listener. The composition root
/// (`tdmcp-daemon`) resolves env/config into this before binding; `tdmcp-ipc`
/// intentionally knows nothing about config precedence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeEndpoint {
    /// TCP loopback listener (v0 transport; loopback-only per D2).
    Tcp {
        /// Bind host — must be loopback.
        host: String,
        /// Bind port (default 9861).
        port: u16,
    },
}

/// Accepted connection after handshake.
pub struct IpcStream {
    /// Connected peer pid from handshake.
    pub pid: u32,
    /// Handshake request attrs.
    pub handshake: HandshakeRequest,
    transport: Transport,
}

enum Transport {
    Tcp(tokio::net::TcpStream),
    /// Duplex used by tests.
    Memory(tokio::io::DuplexStream),
}

impl IpcStream {
    /// Wrap a memory duplex that already completed handshake (tests).
    #[must_use]
    pub fn from_memory(stream: tokio::io::DuplexStream, handshake: HandshakeRequest) -> Self {
        Self {
            pid: handshake.pid,
            handshake,
            transport: Transport::Memory(stream),
        }
    }

    /// Perform handshake over a memory duplex pair end.
    pub async fn accept_memory_handshake(
        stream: tokio::io::DuplexStream,
        bridge_package_dir: impl Into<String>,
        daemon_version: impl Into<String>,
    ) -> Result<Self, IpcError> {
        Self::accept_memory_handshake_with(
            stream,
            bridge_package_dir,
            daemon_version,
            HandshakeOffer::default(),
        )
        .await
    }

    /// Memory handshake with optional budgets forwarded to the peer.
    pub async fn accept_memory_handshake_with(
        stream: tokio::io::DuplexStream,
        bridge_package_dir: impl Into<String>,
        daemon_version: impl Into<String>,
        offer: HandshakeOffer,
    ) -> Result<Self, IpcError> {
        let (req, stream) = complete_handshake_io(
            stream,
            bridge_package_dir.into(),
            daemon_version.into(),
            offer,
        )
        .await?;
        Ok(Self::from_memory(stream, req))
    }

    /// Write a framed message.
    pub async fn send<T: serde::Serialize>(&mut self, msg: &T) -> Result<(), IpcError> {
        let bytes = framing::encode(msg)?;
        match &mut self.transport {
            Transport::Tcp(t) => t.write_all(&bytes).await?,
            Transport::Memory(m) => m.write_all(&bytes).await?,
        }
        Ok(())
    }

    /// Read a typed [`Message`].
    pub async fn recv_message(&mut self) -> Result<Message, IpcError> {
        match &mut self.transport {
            Transport::Tcp(t) => read_msg(t).await,
            Transport::Memory(m) => read_msg(m).await,
        }
    }
}

/// TCP listener for handshaken bridge peers.
pub struct IpcListener {
    endpoint: BridgeEndpoint,
    listener: TcpListener,
    addr: SocketAddr,
}

impl IpcListener {
    /// Bind the configured endpoint.
    ///
    /// # Errors
    /// Rejects non-loopback hosts ([`IpcError::NotLoopback`], T-4) and
    /// reports bind conflicts with the offending address
    /// ([`IpcError::Bind`], T-3).
    pub async fn bind(endpoint: BridgeEndpoint) -> Result<Self, IpcError> {
        let BridgeEndpoint::Tcp { host, port } = &endpoint;
        let addr = resolve_loopback(host, *port)?;
        let listener =
            TcpListener::bind(addr)
                .await
                .map_err(|source| IpcError::Bind { addr, source })?;
        // Report the bound address (port 0 binds get the OS-assigned port).
        let addr = listener.local_addr()?;
        info!(%addr, "binding tcp bridge listener");
        Ok(Self {
            endpoint,
            listener,
            addr,
        })
    }

    /// Endpoint in use (as configured; see [`local_addr`](Self::local_addr)
    /// for the bound address, e.g. after a port-0 test bind).
    #[must_use]
    pub fn endpoint(&self) -> &BridgeEndpoint {
        &self.endpoint
    }

    /// Bound socket address.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Accept one connection and complete handshake.
    ///
    /// Post-connect handshake I/O is bounded by [`HANDSHAKE_IO_TIMEOUT`].
    /// Rejected peers (garbled frame, unsupported `protocol_version`, missing
    /// frames) receive one framed JSON error, then the connection closes.
    pub async fn accept_handshake(
        &self,
        bridge_package_dir: impl Into<String>,
        daemon_version: impl Into<String>,
        offer: HandshakeOffer,
    ) -> Result<IpcStream, IpcError> {
        let (stream, _peer) = self.listener.accept().await?;
        let (req, stream) = complete_handshake_io(
            stream,
            bridge_package_dir.into(),
            daemon_version.into(),
            offer,
        )
        .await?;
        Ok(IpcStream {
            pid: req.pid,
            handshake: req,
            transport: Transport::Tcp(stream),
        })
    }
}

/// Map a configured host to a loopback [`SocketAddr`] (T-4).
///
/// `localhost` maps to `127.0.0.1` (deterministic IPv4 loopback; `::1` could
/// fail on hosts without IPv6). Any other name or address must parse to a
/// loopback IP.
fn resolve_loopback(host: &str, port: u16) -> Result<SocketAddr, IpcError> {
    let ip: IpAddr = if host.eq_ignore_ascii_case("localhost") {
        IpAddr::from([127, 0, 0, 1])
    } else {
        host.parse()
            .map_err(|_| IpcError::NotLoopback(host.to_owned()))?
    };
    if !ip.is_loopback() {
        return Err(IpcError::NotLoopback(host.to_owned()));
    }
    Ok(SocketAddr::from((ip, port)))
}

/// Why a handshake was rejected, before the rejection frame is written.
enum HandshakeFailure {
    /// Peer sent nothing readable within the budget.
    Timeout,
    /// First frame unreadable/garbled, or the response write failed.
    Invalid(IpcError),
    /// Bridge speaks an unsupported `protocol_version` (D5: pipe-era tox).
    Version(String),
}

impl HandshakeFailure {
    /// Wire/log code + human message; both carry the re-embed hint (D5).
    fn rejection(&self) -> (&'static str, String) {
        match self {
            Self::Timeout => (
                HANDSHAKE_TIMEOUT_CODE,
                format!(
                    "bridge sent no handshake frame within {HANDSHAKE_IO_TIMEOUT:?}; \
                     re-embed the shipped bootstrap tox"
                ),
            ),
            Self::Invalid(e) => (
                HANDSHAKE_INVALID_CODE,
                format!("bridge handshake frame unreadable: {e}; re-embed the shipped bootstrap tox"),
            ),
            Self::Version(v) => (
                PROTOCOL_MISMATCH_CODE,
                format!(
                    "bridge protocol_version {v:?} not supported, daemon speaks \
                     {PROTOCOL_VERSION}; re-embed the shipped bootstrap tox"
                ),
            ),
        }
    }

    /// Error the accept loop observes for an already-rejected connection.
    fn into_ipc_error(self) -> IpcError {
        match self {
            Self::Timeout => IpcError::HandshakeTimeout(HANDSHAKE_IO_TIMEOUT),
            Self::Invalid(e) => e,
            Self::Version(_) => IpcError::Handshake(self.rejection().1),
        }
    }
}

/// Read handshake request + write response under [`HANDSHAKE_IO_TIMEOUT`]
/// (one connection, one handshake). On any rejection the peer gets one framed
/// JSON error, then the connection closes (T-6).
async fn complete_handshake_io<S>(
    mut stream: S,
    bridge_package_dir: String,
    daemon_version: String,
    offer: HandshakeOffer,
) -> Result<(HandshakeRequest, S), IpcError>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let outcome = timeout(
        HANDSHAKE_IO_TIMEOUT,
        handshake_exchange(&mut stream, &bridge_package_dir, &daemon_version, offer),
    )
    .await;
    match outcome {
        Ok(Ok(req)) => Ok((req, stream)),
        Ok(Err(failure)) => Err(reject_handshake(&mut stream, failure).await),
        Err(_) => Err(reject_handshake(&mut stream, HandshakeFailure::Timeout).await),
    }
}

/// The handshake exchange proper: read request, validate version, write
/// response. Runs inside the single handshake budget.
async fn handshake_exchange<S>(
    stream: &mut S,
    bridge_package_dir: &str,
    daemon_version: &str,
    offer: HandshakeOffer,
) -> Result<HandshakeRequest, HandshakeFailure>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let req: HandshakeRequest =
        read_msg(stream).await.map_err(HandshakeFailure::Invalid)?;
    if req.protocol_version != PROTOCOL_VERSION {
        return Err(HandshakeFailure::Version(req.protocol_version));
    }
    let resp = HandshakeResponse {
        bridge_package_dir: bridge_package_dir.to_owned(),
        daemon_version: daemon_version.to_owned(),
        min_daemon: None,
        idle_dead_secs: offer.idle_dead_secs,
        max_call_wait_secs: offer.max_call_wait_secs,
    };
    write_msg(stream, &resp)
        .await
        .map_err(HandshakeFailure::Invalid)?;
    Ok(req)
}

/// Log the rejection under `tdmcp.bridge.*`, write one framed error, close.
async fn reject_handshake<S>(stream: &mut S, failure: HandshakeFailure) -> IpcError
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let (code, message) = failure.rejection();
    // Message leads with the wire code so `tdmcp.bridge.*` is greppable in
    // daemon logs (D5); the dotted form stays out of the tracing target,
    // which follows the crate's `tdmcp_*` target convention.
    warn!(code, "{code}: {message}");
    let err = serde_json::json!({"ok": false, "code": code, "message": message});
    // Best-effort and bounded: the peer may already be gone or stuck.
    let _ = timeout(ERROR_FRAME_WRITE_BUDGET, write_msg(stream, &err)).await;
    // FIN first, then drain unread inbound bytes: closing a socket that still
    // has unread data sends RST, which discards the error frame client-side.
    let _ = stream.shutdown().await;
    let _ = timeout(ERROR_FRAME_WRITE_BUDGET, async {
        let mut sink = [0u8; 4096];
        loop {
            match stream.read(&mut sink).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
    .await;
    failure.into_ipc_error()
}

async fn read_msg<R: AsyncReadExt + Unpin, T: serde::de::DeserializeOwned>(
    r: &mut R,
) -> Result<T, IpcError> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(IpcError::Frame(FrameError::TooLarge(len)));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body).map_err(FrameError::Json)?)
}

async fn write_msg<W: AsyncWriteExt + Unpin, T: serde::Serialize>(
    w: &mut W,
    msg: &T,
) -> Result<(), IpcError> {
    let bytes = framing::encode(msg)?;
    w.write_all(&bytes).await?;
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests"
)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// `expect_err` needs `T: Debug`; `IpcStream` has none by design.
    fn expect_reject(r: Result<IpcStream, IpcError>) -> IpcError {
        match r {
            Ok(_) => panic!("handshake must be rejected"),
            Err(e) => e,
        }
    }

    #[tokio::test]
    async fn memory_handshake_times_out_when_peer_silent() {
        let (_client, server) = tokio::io::duplex(64 * 1024);
        let start = Instant::now();
        match IpcStream::accept_memory_handshake(server, "/bridge", "0.1.0").await {
            Err(IpcError::HandshakeTimeout(_)) => {}
            Ok(_) => panic!("silent peer must not complete handshake"),
            Err(other) => panic!("expected HandshakeTimeout, got {other}"),
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed >= HANDSHAKE_IO_TIMEOUT,
            "elapsed {elapsed:?} shorter than timeout"
        );
        assert!(
            elapsed < HANDSHAKE_IO_TIMEOUT + Duration::from_secs(3),
            "elapsed {elapsed:?} hung past timeout budget"
        );
    }

    /// Read one framed JSON value, then assert the connection closes.
    async fn read_error_then_eof<T: serde::de::DeserializeOwned>(client: &mut tokio::io::DuplexStream) -> T {
        let mut len_buf = [0u8; 4];
        client.read_exact(&mut len_buf).await.expect("error frame length");
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut body = vec![0u8; len];
        client.read_exact(&mut body).await.expect("error frame body");
        let parsed: T = serde_json::from_slice(&body).expect("error frame json");
        let mut eof = [0u8; 1];
        assert_eq!(
            client.read(&mut eof).await.expect("eof read"),
            0,
            "connection must close after the error frame"
        );
        parsed
    }

    #[tokio::test]
    async fn garbage_first_frame_gets_framed_error_then_close() {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(async move {
            IpcStream::accept_memory_handshake(server, "/bridge", "0.1.0").await
        });
        client
            .write_all(b"GET / HTTP/1.1 rubbish")
            .await
            .expect("write garbage");
        let err = match server_task.await.expect("server task") {
            Ok(_) => panic!("garbage peer must not complete handshake"),
            Err(e) => e,
        };
        assert!(matches!(err, IpcError::Frame(_)), "got {err}");
        let value: serde_json::Value = read_error_then_eof(&mut client).await;
        assert_eq!(value["ok"], false);
        assert_eq!(value["code"], HANDSHAKE_INVALID_CODE);
        assert!(value["message"].as_str().unwrap().contains("re-embed"));
    }

    #[tokio::test]
    async fn unsupported_protocol_version_gets_mismatch_error_then_close() {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(async move {
            IpcStream::accept_memory_handshake(server, "/bridge", "0.1.0").await
        });
        let req = HandshakeRequest {
            pid: 7,
            protocol_version: "999".into(),
            title: None,
            toe_path: None,
            image: None,
            start_time: None,
        };
        let bytes = framing::encode(&req).expect("encode handshake");
        client.write_all(&bytes).await.expect("write handshake");
        let err = expect_reject(server_task.await.expect("server task"));
        assert!(matches!(err, IpcError::Handshake(_)), "got {err}");
        let value: serde_json::Value = read_error_then_eof(&mut client).await;
        assert_eq!(value["ok"], false);
        assert_eq!(value["code"], PROTOCOL_MISMATCH_CODE);
    }

    #[test]
    fn loopback_resolution_accepts_loopback_only() {
        assert_eq!(
            resolve_loopback("127.0.0.1", 9861).expect("v4 loopback"),
            SocketAddr::from(([127, 0, 0, 1], 9861))
        );
        assert_eq!(
            resolve_loopback("::1", 1).expect("v6 loopback"),
            SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 1))
        );
        assert_eq!(
            resolve_loopback("localhost", 5).expect("localhost maps to v4 loopback"),
            SocketAddr::from(([127, 0, 0, 1], 5))
        );
        for host in ["0.0.0.0", "192.168.1.9", "example.com", ""] {
            assert!(
                matches!(
                    resolve_loopback(host, 9861),
                    Err(IpcError::NotLoopback(_))
                ),
                "{host:?} must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn bind_conflict_error_names_host_and_port() {
        let first = IpcListener::bind(BridgeEndpoint::Tcp {
            host: "127.0.0.1".into(),
            port: 0,
        })
        .await
        .expect("first bind");
        let port = first.local_addr().port();
        let err = match IpcListener::bind(BridgeEndpoint::Tcp {
            host: "127.0.0.1".into(),
            port,
        })
        .await
        {
            Ok(_) => panic!("second bind on a live port must fail"),
            Err(e) => e,
        };
        let text = err.to_string();
        assert!(
            text.contains("127.0.0.1") && text.contains(&port.to_string()),
            "bind error must name host:port, got: {text}"
        );
    }
}
