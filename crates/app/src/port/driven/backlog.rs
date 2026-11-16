//! The backlog: how much work waits, and for how long (ADR 0056).

use std::fmt;

/// What waits in one queue.
#[derive(Clone, PartialEq, Debug)]
pub struct QueueBacklog {
    /// `job` for a check job, `run` for a run.
    pub kind: &'static str,
    /// The labels a runner needs to take it, joined by commas in order.
    pub labels: String,
    /// How many are queued.
    pub depth: u64,
    /// How many seconds the longest-waiting one has been queued.
    pub oldest_age_seconds: f64,
}

/// What waits now.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct BacklogReading {
    /// The queued jobs and runs, one entry per kind and label set that has any.
    pub queues: Vec<QueueBacklog>,
    /// The repository changes the directory outbox has not yet delivered.
    pub outbox_pending: u64,
}

/// Reads the backlog.
#[async_trait::async_trait]
pub trait Backlog: Send + Sync {
    /// What waits now.
    ///
    /// # Errors
    ///
    /// [`BacklogError`] where the backlog could not be read.
    async fn read(&self) -> Result<BacklogReading, BacklogError>;
}

/// Why the backlog could not be read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BacklogError(pub String);

impl fmt::Display for BacklogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "reading the backlog failed: {}", self.0)
    }
}

impl std::error::Error for BacklogError {}
