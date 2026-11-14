//! Jobs: one check of one changeset, queued for a runner to take (ADR 0034).

use std::fmt;

use crate::domain::address::Address;
use crate::domain::check::{CheckDefinition, CheckKey};
use crate::domain::file::File;
use crate::domain::name::OrganisationName;
use crate::domain::path::{Path, Prefix};
use crate::domain::repository::RepositoryId;
use crate::domain::revert::Revert;
use crate::domain::runner::RegistrationId;
use crate::domain::tree::Tree;

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
    /// Waiting for the checks it needs to pass, so that it can be keyed; not yet for a runner.
    Waiting,
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
    /// A check it needs did not pass, so it was never queued. It has not passed.
    Skipped,
    /// Its key had already passed, so it was not queued and takes the result of the run that
    /// passed.
    Cached,
}

impl JobState {
    /// Whether the job has ended, so that it will never run again.
    #[must_use]
    pub const fn is_finished(self) -> bool {
        match self {
            Self::Waiting | Self::Queued | Self::Running { .. } => false,
            Self::Passed
            | Self::Failed
            | Self::TimedOut
            | Self::Cancelled
            | Self::Skipped
            | Self::Cached => true,
        }
    }

    /// Whether the check passed, by a run or from the cache.
    #[must_use]
    pub const fn is_passed(self) -> bool {
        matches!(self, Self::Passed | Self::Cached)
    }
}

/// What becomes of a waiting job once the checks it needs have ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resolution {
    /// Queue it for a runner, under `key`.
    Queue {
        /// The key of the check.
        key: CheckKey,
    },
    /// Record it as passed from the cache: `key` already passed, with `output_tree`.
    Cache {
        /// The key of the check.
        key: CheckKey,
        /// The output tree of the run that passed.
        output_tree: Address,
    },
    /// Record it as skipped: a check it needs did not pass.
    Skip,
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
    /// What the result depends on; set once the checks the job needs have passed, and `None` for
    /// a job that was skipped or recorded as failed before it could be keyed.
    pub key: Option<CheckKey>,
    /// The tree the run recorded as its output; set where the job passed or took a cached result.
    pub output_tree: Option<Address>,
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
    /// The runner enforced the check's timeout and stopped it.
    TimedOut,
    /// The runner stopped the check because it was cancelled.
    Cancelled,
}

impl Outcome {
    /// The state a job ends in when a runner reports this outcome.
    #[must_use]
    pub const fn state(self) -> JobState {
        match self {
            Self::Passed => JobState::Passed,
            Self::Failed => JobState::Failed,
            Self::TimedOut => JobState::TimedOut,
            Self::Cancelled => JobState::Cancelled,
        }
    }
}

/// What a runner reports when its job ends.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct JobResult {
    /// How it ended.
    pub outcome: Outcome,
    /// The command's exit code; `None` where the runner never started it.
    pub exit_code: Option<i32>,
    /// The command's output; only its tail is kept.
    pub output: Vec<u8>,
}

/// A file a job left under its `outputs` or `caches`, which the runner has uploaded.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProducedFile {
    /// Where the file is in the repository, from the root.
    pub path: Path,
    /// The file's mode and the address of its bytes.
    pub file: File,
}

/// The files a job left, which a runner reports with its result.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Produced {
    /// The files under the check's `outputs`.
    pub outputs: Vec<ProducedFile>,
    /// The files under the check's `caches`.
    pub caches: Vec<ProducedFile>,
}

/// What a cache ref records: the tree of a cache, and the changeset whose job left it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CacheRef {
    /// The tree of the cache, rooted at the cache path.
    pub tree: Address,
    /// The landed changeset of the job that left the tree.
    pub changeset: Address,
}

impl Job {
    /// The paths the check declares as outputs, from the repository root.
    #[must_use]
    pub fn output_prefixes(&self) -> Vec<Prefix> {
        self.check
            .outputs
            .iter()
            .map(|path| under(&self.check.folder, path))
            .collect()
    }

    /// The paths the check declares as caches, from the repository root.
    #[must_use]
    pub fn cache_prefixes(&self) -> Vec<Prefix> {
        self.check
            .caches
            .iter()
            .map(|path| under(&self.check.folder, path))
            .collect()
    }
}

/// `path` below `folder`, from the repository root.
fn under(folder: &Prefix, path: &Path) -> Prefix {
    let mut components = folder.components().to_vec();
    components.extend_from_slice(path.components());
    Prefix::new(components)
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

/// The output tree of a run that records none: the empty tree, which a run records where it
/// passed with no output, or did not pass.
#[must_use]
pub fn empty_output_tree() -> Address {
    // An empty tree always has an encoding, so the fallback is never taken.
    Tree::default()
        .address()
        .unwrap_or_else(|_| Address::of(&[]))
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
        assert!(!JobState::Waiting.is_finished());
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
        assert!(JobState::Skipped.is_finished());
        assert!(JobState::Cached.is_finished());
    }

    #[test]
    fn only_a_run_that_passed_or_a_cached_result_is_passed() {
        assert!(JobState::Passed.is_passed());
        assert!(JobState::Cached.is_passed());
        assert!(!JobState::Skipped.is_passed());
        assert!(!JobState::Failed.is_passed());
        assert!(!JobState::Waiting.is_passed());
    }
}
