//! Cross-platform local IPC listener (named pipe on Windows, UDS elsewhere).

use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use tracing::{debug, info};

use crate::framing::{self, FrameError, Message, MAX_FRAME};
use crate::handshake::{HandshakeOffer, HandshakeRequest, HandshakeResponse};

/// Budget for post-connect handshake frame read + response write.
pub const HANDSHAKE_IO_TIMEOUT: Duration = Duration::from_secs(5);

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
}

/// Resolved endpoint for this platform.
#[derive(Debug, Clone)]
pub enum BridgeEndpoint {
    /// Windows named pipe path (`\\.\pipe\tdmcp-rs`).
    #[cfg(windows)]
    NamedPipe(String),
    /// Unix domain socket path.
    #[cfg(unix)]
    UnixSocket(PathBuf),
}

impl BridgeEndpoint {
    /// Default production endpoint.
    ///
    /// On Windows, `TDMCP_IPC_PIPE` overrides the named-pipe path (tests use a
    /// unique pipe so a live TD on `\\.\pipe\tdmcp-rs` cannot attach).
    #[must_use]
    pub fn default_endpoint(data_dir: &Path) -> Self {
        #[cfg(windows)]
        {
            let _ = data_dir;
            let name =
                std::env::var("TDMCP_IPC_PIPE").unwrap_or_else(|_| r"\\.\pipe\tdmcp-rs".to_owned());
            Self::NamedPipe(name)
        }
        #[cfg(unix)]
        {
            Self::UnixSocket(data_dir.join("bridge.sock"))
        }
    }
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
    #[cfg(windows)]
    Pipe(tokio::net::windows::named_pipe::NamedPipeServer),
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
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
            #[cfg(windows)]
            Transport::Pipe(p) => p.write_all(&bytes).await?,
            #[cfg(unix)]
            Transport::Unix(u) => u.write_all(&bytes).await?,
            Transport::Memory(m) => m.write_all(&bytes).await?,
        }
        Ok(())
    }

    /// Read a typed [`Message`].
    pub async fn recv_message(&mut self) -> Result<Message, IpcError> {
        match &mut self.transport {
            #[cfg(windows)]
            Transport::Pipe(p) => read_msg(p).await,
            #[cfg(unix)]
            Transport::Unix(u) => read_msg(u).await,
            Transport::Memory(m) => read_msg(m).await,
        }
    }
}

/// Platform listener.
pub struct IpcListener {
    endpoint: BridgeEndpoint,
    #[cfg(unix)]
    uds: tokio::net::UnixListener,
    #[cfg(windows)]
    first: std::sync::atomic::AtomicBool,
}

impl IpcListener {
    /// Bind the configured endpoint.
    pub async fn bind(endpoint: BridgeEndpoint) -> Result<Self, IpcError> {
        #[cfg(windows)]
        {
            let BridgeEndpoint::NamedPipe(ref name) = endpoint;
            info!(%name, "named pipe endpoint configured");
            Ok(Self {
                endpoint,
                first: std::sync::atomic::AtomicBool::new(true),
            })
        }
        #[cfg(unix)]
        {
            let BridgeEndpoint::UnixSocket(path) = &endpoint else {
                return Err(IpcError::Handshake("expected unix socket endpoint".into()));
            };
            if path.exists() {
                let _ = std::fs::remove_file(path);
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            info!(path = %path.display(), "binding unix socket");
            let uds = tokio::net::UnixListener::bind(path)?;
            Ok(Self { endpoint, uds })
        }
    }

    /// Endpoint in use.
    #[must_use]
    pub fn endpoint(&self) -> &BridgeEndpoint {
        &self.endpoint
    }

    /// Accept one connection and complete handshake.
    ///
    /// Post-connect handshake I/O is bounded by [`HANDSHAKE_IO_TIMEOUT`].
    pub async fn accept_handshake(
        &self,
        bridge_package_dir: impl Into<String>,
        daemon_version: impl Into<String>,
        offer: HandshakeOffer,
    ) -> Result<IpcStream, IpcError> {
        let bridge_package_dir = bridge_package_dir.into();
        let daemon_version = daemon_version.into();

        #[cfg(windows)]
        {
            use std::sync::atomic::Ordering;
            use tokio::net::windows::named_pipe::ServerOptions;
            let BridgeEndpoint::NamedPipe(ref name) = self.endpoint;
            let first = self.first.load(Ordering::SeqCst);
            let server = ServerOptions::new()
                .first_pipe_instance(first)
                .create(name)?;
            // Only spend the first-instance claim after create succeeds.
            self.first.store(false, Ordering::SeqCst);
            debug!(%name, first, "waiting for named pipe client");
            server.connect().await?;
            let (req, server) =
                complete_handshake_io(server, bridge_package_dir, daemon_version, offer).await?;
            Ok(IpcStream {
                pid: req.pid,
                handshake: req,
                transport: Transport::Pipe(server),
            })
        }

        #[cfg(unix)]
        {
            let (stream, _) = self.uds.accept().await?;
            let (req, stream) =
                complete_handshake_io(stream, bridge_package_dir, daemon_version, offer).await?;
            Ok(IpcStream {
                pid: req.pid,
                handshake: req,
                transport: Transport::Unix(stream),
            })
        }
    }
}

/// Read handshake request + write response under [`HANDSHAKE_IO_TIMEOUT`].
async fn complete_handshake_io<S>(
    mut stream: S,
    bridge_package_dir: String,
    daemon_version: String,
    offer: HandshakeOffer,
) -> Result<(HandshakeRequest, S), IpcError>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    match timeout(HANDSHAKE_IO_TIMEOUT, async {
        let req: HandshakeRequest = read_msg(&mut stream).await?;
        let resp = HandshakeResponse {
            bridge_package_dir,
            daemon_version,
            min_daemon: None,
            idle_dead_secs: offer.idle_dead_secs,
            max_call_wait_secs: offer.max_call_wait_secs,
        };
        write_msg(&mut stream, &resp).await?;
        Ok::<_, IpcError>((req, stream))
    })
    .await
    {
        Ok(inner) => inner,
        Err(_) => Err(IpcError::HandshakeTimeout(HANDSHAKE_IO_TIMEOUT)),
    }
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
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;
    use std::time::Instant;

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
}
