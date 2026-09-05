//! Single-flight, bounded background work for desktop interactions.

use std::sync::mpsc::{self, Receiver, TryRecvError};

pub(crate) struct Background<T> {
    receiver: Option<Receiver<Result<T, String>>>,
}

impl<T> Default for Background<T> {
    fn default() -> Self {
        Self { receiver: None }
    }
}

impl<T: Send + 'static> Background<T> {
    pub(crate) fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    pub(crate) fn start(
        &mut self,
        name: &str,
        work: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<(), String> {
        if self.is_running() {
            return Err("An operation is already running".into());
        }
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let _ = sender.send(work());
            })
            .map_err(|e| e.to_string())?;
        self.receiver = Some(receiver);
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> Option<Result<T, String>> {
        let result = match self.receiver.as_ref()?.try_recv() {
            Ok(value) => value,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("Background worker stopped".into()),
        };
        self.receiver = None;
        Some(result)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test assertions")]
mod tests {
    use super::*;

    #[test]
    fn refuses_duplicate_work_and_can_be_reused() {
        let mut job = Background::default();
        let (release, wait) = mpsc::channel();
        job.start("test-worker", move || {
            wait.recv().map_err(|e| e.to_string())
        })
        .expect("start");
        assert!(job.poll().is_none());
        assert!(job.start("duplicate", || Ok(0)).is_err());
        release.send(42).expect("release");
        assert_eq!(
            job.receiver
                .take()
                .expect("receiver")
                .recv()
                .expect("result"),
            Ok(42)
        );
        assert!(!job.is_running());
        job.start("reused", || Ok(7)).expect("restart");
        assert_eq!(
            job.receiver
                .take()
                .expect("receiver")
                .recv()
                .expect("result"),
            Ok(7)
        );
    }
}
