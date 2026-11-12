//! Jobs: one check of one changeset, queued for a runner to take (ADR 0034).

use std::fmt;

use crate::domain::address::Address;
use crate::domain::check::CheckDefinition;
use crate::domain::name::OrganisationName;
use crate::domain::repository::RepositoryId;
use crate::domain::revert::Revert;
use crate::domain::runner::RegistrationId;

/// The most output a job keeps, in bytes: the tail of what the runner reported.
pub const OUTPUT_LIMIT: usize = 65_536;

/// A job: opaque, assigned by the job queue.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct JobId(u64);

impl JobId {
    /// The identifier a queue assigns from `value`; distinct values are distinct jobs.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The number the identifier was made from; [`JobId::new`] of it is this identifier.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Where a job is in its life.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobState {
    /// Waiting for a runner.
    Queued,
    /// A runner holds it.
    Running {
        /// When the runner's lease lapses unless it heartbeats, in seconds since
        /// 1970-01-01T00:00:00Z. A lapsed lease puts the job back in the queue.
        lease_expires_at: i64,
        /// When the job times out, in seconds since 1970-01-01T00:00:00Z. Heartbeats do not move
        /// it.
        deadline: i64,
    },
    /// The runner reported success.
    Passed,
    /// The runner reported failure, or the check definitions could not be read.
    Failed,
    /// The deadline passed before the runner reported.
    TimedOut,
    /// An administrator cancelled the job.
    Cancelled,
}

impl JobState {
    /// Whether the job has ended, so that it will never run again.
    #[must_use]
    pub const fn is_finished(self) -> bool {
        match self {
            Self::Queued | Self::Running { .. } => false,
            Self::Passed | Self::Failed | Self::TimedOut | Self::Cancelled => true,
        }
    }
}

/// What a job says before the queue has given it an identifier.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NewJob {
    /// The organisation of the repository.
    pub organisation: OrganisationName,
    /// The repository the changeset is in.
    pub repository: RepositoryId,
    /// The landed changeset the check runs against.
    pub changeset: Address,
    /// The check.
    pub check: CheckDefinition,
}

/// A job as the queue holds it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Job {
    /// Names the job.
    pub id: JobId,
    /// The organisation of the repository.
    pub organisation: OrganisationName,
    /// The repository the changeset is in.
    pub repository: RepositoryId,
    /// The landed changeset the check runs against.
    pub changeset: Address,
    /// The check.
    pub check: CheckDefinition,
    /// Where the job is.
    pub state: JobState,
    /// The registration that took the job; set from the claim on, and cleared where the lease
    /// lapses and the job returns to the queue.
    pub runner: Option<RegistrationId>,
    /// The exit code the runner reported.
    pub exit_code: Option<i32>,
    /// The tail of the output the runner reported, at most [`OUTPUT_LIMIT`] bytes.
    pub output: Vec<u8>,
    /// What came of reverting the changeset because this check failed or timed out; `None` where
    /// no revert was attempted for this job.
    pub revert: Option<Revert>,
}

/// How a runner says a job ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The check passed.
    Passed,
    /// The check failed.
    Failed,
}

impl Outcome {
    /// The state a job ends in when a runner reports this outcome.
    #[must_use]
    pub const fn state(self) -> JobState {
        match self {
            Self::Passed => JobState::Passed,
            Self::Failed => JobState::Failed,
        }
    }
}

/// What a runner reports when its job ends.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct JobResult {
    /// How it ended.
    pub outcome: Outcome,
    /// The command's exit code.
    pub exit_code: i32,
    /// The command's output; only its tail is kept.
    pub output: Vec<u8>,
}

/// Why a runner should stop a job it still holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StopReason {
    /// An administrator cancelled the job.
    Cancelled,
    /// The job passed its deadline.
    TimedOut,
}

/// What a runner learns from a heartbeat.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Heartbeat {
    /// The lease is extended; keep running.
    Continue,
    /// Stop: the job is over.
    Stop(StopReason),
}

/// The last [`OUTPUT_LIMIT`] bytes of `output`.
#[must_use]
pub fn output_tail(output: &[u8]) -> Vec<u8> {
    let start = output.len().saturating_sub(OUTPUT_LIMIT);
    output.get(start..).unwrap_or_default().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_end_of_output_past_the_limit() {
        let mut output = vec![b'a'; OUTPUT_LIMIT];
        output.push(b'z');
        let tail = output_tail(&output);
        assert_eq!(tail.len(), OUTPUT_LIMIT);
        assert_eq!(tail.last(), Some(&b'z'));
        assert_eq!(tail.first(), Some(&b'a'));
        assert_eq!(output_tail(b"short"), b"short".to_vec());
    }

    #[test]
    fn only_the_ended_states_are_finished() {
        assert!(!JobState::Queued.is_finished());
        assert!(
            !JobState::Running {
                lease_expires_at: 0,
                deadline: 0
            }
            .is_finished()
        );
        assert!(JobState::Passed.is_finished());
        assert!(JobState::Failed.is_finished());
        assert!(JobState::TimedOut.is_finished());
        assert!(JobState::Cancelled.is_finished());
    }
}
