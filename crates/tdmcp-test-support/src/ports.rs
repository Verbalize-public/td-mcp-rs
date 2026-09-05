//! Port selection for child-process integration tests.

use std::collections::BTreeSet;
use std::io;
use std::net::TcpListener;
use std::sync::Mutex;

static SELECTED: Mutex<BTreeSet<u16>> = Mutex::new(BTreeSet::new());

/// Select a loopback port never previously returned in this test process.
///
/// Binding port zero and immediately dropping the listener lets parallel
/// tests pick the same port before either child binds it. Remember selections
/// for the process lifetime to prevent that race between users of this helper.
/// This is not an OS reservation: readiness checks must still verify the child
/// PID, since an unrelated program can bind during the handoff.
///
/// # Errors
/// Returns socket errors, poisoned allocator state, or selection exhaustion.
pub fn unique_test_port() -> io::Result<u16> {
    let mut selected = SELECTED
        .lock()
        .map_err(|_| io::Error::other("test port allocator poisoned"))?;
    for _ in 0..256 {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        if selected.insert(port) {
            return Ok(port);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AddrNotAvailable,
        "could not select an unused test port after 256 attempts",
    ))
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test assertions")]
mod tests {
    use super::*;

    #[test]
    fn concurrent_selections_never_reuse_a_port() {
        let threads: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(|| {
                    (0..32)
                        .map(|_| unique_test_port().expect("port"))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let ports: Vec<_> = threads
            .into_iter()
            .flat_map(|thread| thread.join().expect("port selector thread"))
            .collect();
        assert_eq!(ports.len(), 256);
        assert_eq!(ports.iter().copied().collect::<BTreeSet<_>>().len(), 256);
        assert!(!ports.contains(&0));
    }
}
