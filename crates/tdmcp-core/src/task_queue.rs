//! Per-pid task queue: shared (default) vs exclusive.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Shared vs exclusive enqueue mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskMode {
    /// Enqueue behind existing work.
    Shared,
    /// Fail if the queue is non-empty (any shared or exclusive).
    Exclusive,
}

/// Outcome recorded after a task finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskResult {
    /// Completed successfully.
    Success,
    /// Failed (domain / bridge / script).
    Failed,
}

/// Visible task descriptor for `fleet`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    /// Short name (e.g. `PythonEval`, `MutateNodes`).
    pub name: String,
    /// Whether the task requested exclusive.
    pub exclusive: bool,
}

/// Queue errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum QueueError {
    /// Exclusive request while queue non-empty.
    #[error("queue_busy: exclusive rejected while {len} task(s) present")]
    Busy {
        /// Current queue length.
        len: usize,
    },
    /// No in-flight task to complete.
    #[error("no in-flight task")]
    NoInFlight,
}

/// Per-pid FIFO of tasks with exclusive semantics.
#[derive(Debug, Default)]
pub struct TaskQueue {
    /// Queued (not yet started) + optionally the in-flight head is tracked separately.
    pending: Vec<TaskInfo>,
    in_flight: Option<TaskInfo>,
}

impl TaskQueue {
    /// Create an empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of pending + in-flight tasks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len() + usize::from(self.in_flight.is_some())
    }

    /// True if nothing is queued or running.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Snapshot for `fleet` (`include: tasks`).
    #[must_use]
    pub fn snapshot(&self) -> Vec<TaskInfo> {
        let mut out = Vec::with_capacity(self.len());
        if let Some(t) = &self.in_flight {
            out.push(t.clone());
        }
        out.extend(self.pending.iter().cloned());
        out
    }

    /// Try to enqueue. Exclusive fails if queue is **non-empty**.
    pub fn try_enqueue(
        &mut self,
        name: impl Into<String>,
        mode: TaskMode,
    ) -> Result<(), QueueError> {
        let exclusive = matches!(mode, TaskMode::Exclusive);
        if exclusive && !self.is_empty() {
            return Err(QueueError::Busy { len: self.len() });
        }
        self.pending.push(TaskInfo {
            name: name.into(),
            exclusive,
        });
        Ok(())
    }

    /// Promote the next pending task to in-flight if none is running.
    pub fn start_next(&mut self) -> Option<&TaskInfo> {
        if self.in_flight.is_some() {
            return self.in_flight.as_ref();
        }
        if self.pending.is_empty() {
            return None;
        }
        self.in_flight = Some(self.pending.remove(0));
        self.in_flight.as_ref()
    }

    /// Complete the in-flight task.
    pub fn complete_in_flight(&mut self, _result: TaskResult) -> Result<TaskInfo, QueueError> {
        self.in_flight.take().ok_or(QueueError::NoInFlight)
    }

    /// Cancel all queued and in-flight work (bridge loss). Returns cancelled infos.
    pub fn cancel_all(&mut self) -> Vec<TaskInfo> {
        let mut out = Vec::new();
        if let Some(t) = self.in_flight.take() {
            out.push(t);
        }
        out.append(&mut self.pending);
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "unit tests")]
mod tests {
    use super::*;

    #[test]
    fn exclusive_fails_when_shared_queued() {
        let mut q = TaskQueue::new();
        q.try_enqueue("SharedA", TaskMode::Shared).unwrap();
        let err = q
            .try_enqueue("ExclusiveB", TaskMode::Exclusive)
            .unwrap_err();
        assert_eq!(err, QueueError::Busy { len: 1 });
    }

    #[test]
    fn exclusive_ok_when_empty() {
        let mut q = TaskQueue::new();
        q.try_enqueue("ExclusiveA", TaskMode::Exclusive).unwrap();
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn shared_may_enqueue_behind_exclusive_in_flight() {
        let mut q = TaskQueue::new();
        q.try_enqueue("ExclusiveA", TaskMode::Exclusive).unwrap();
        q.start_next();
        // Shared may enqueue behind an in-flight exclusive.
        q.try_enqueue("SharedB", TaskMode::Shared).unwrap();
        assert_eq!(q.len(), 2);
        // But another exclusive must fail.
        assert!(matches!(
            q.try_enqueue("ExclusiveC", TaskMode::Exclusive),
            Err(QueueError::Busy { .. })
        ));
    }
}
