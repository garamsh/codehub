//! Runs: a command started in a workspace, queued as an unnamed check on it (ADR 0051).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::check::{Command, Timeout};
use crate::domain::draft::Instance;
use crate::domain::file::File;
use crate::domain::job::ProducedFile;
use crate::domain::name::{DraftName, Label, OrganisationName};
use crate::domain::path::{Path, Prefix};
use crate::domain::principal::Principal;
use crate::domain::repository::RepositoryId;
use crate::domain::runner::RegistrationId;

/// A run: opaque, assigned by the job queue from the sequence its jobs are numbered by, so that a
/// run's id is the id its job is claimed by.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RunId(u64);

impl RunId {
    /// The identifier a queue assigns from `value`; distinct values are distinct runs.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The number the identifier was made from.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What `.codehub/run.toml` says about every run of a workspace; each field has a default where
/// the file or the field is absent.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunSettings {
    /// The cache paths, relative to the repository root, whose workspace cache refs the run uses.
    pub caches: BTreeSet<Path>,
    /// The labels a runner must offer.
    pub labels: BTreeSet<Label>,
    /// How long the run may take.
    pub timeout: Timeout,
}

impl Default for RunSettings {
    fn default() -> Self {
        Self {
            caches: BTreeSet::new(),
            labels: BTreeSet::new(),
            timeout: Timeout::default_timeout(),
        }
    }
}

/// A run as it is queued.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NewRun {
    /// The organisation of the repository.
    pub organisation: OrganisationName,
    /// The repository.
    pub repository: RepositoryId,
    /// The principal who started it and the only one who may read it.
    pub principal: Principal,
    /// The workspace's name.
    pub draft: DraftName,
    /// The opening of the workspace the run is of.
    pub instance: Instance,
    /// The workspace's version when the run started.
    pub version: u64,
    /// The workspace's base changeset.
    pub base: Option<Address>,
    /// What the shell runs.
    pub command: Command,
    /// The directory the files sent are limited to; the whole repository where none is given.
    pub prefix: Prefix,
    /// The paths under the prefix whose files the run's changes may add.
    pub add: BTreeSet<Path>,
    /// The paths the run's output tree holds.
    pub outputs: BTreeSet<Path>,
    /// The labels a runner must offer, from `.codehub/run.toml` when the run started.
    pub labels: BTreeSet<Label>,
    /// How long the run may take, from `.codehub/run.toml` when the run started.
    pub timeout: Timeout,
    /// The files sent: the workspace's tree under the prefix at `version`.
    pub files: BTreeMap<Path, File>,
    /// The cache paths from `.codehub/run.toml`, each with the tree of its workspace cache ref
    /// when the run started.
    pub caches: BTreeMap<Path, Address>,
}

/// Where a run is in its life.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RunState {
    /// Waiting for a runner.
    Queued,
    /// A runner holds it.
    Running {
        /// When the runner's lease lapses, in seconds since 1970-01-01T00:00:00Z.
        lease_expires_at: i64,
        /// When the run times out, in seconds since 1970-01-01T00:00:00Z.
        deadline: i64,
    },
    /// Its principal cancelled it before it ended.
    Cancelled,
    /// Its deadline passed while a runner held it, and the queue ended it.
    TimedOut,
    /// A runner reported how it ended, and the run holds the result.
    Finished,
}

impl RunState {
    /// Whether the run has ended.
    #[must_use]
    pub const fn ended(self) -> bool {
        matches!(self, Self::Cancelled | Self::TimedOut | Self::Finished)
    }
}

/// How a run ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RunOutcome {
    /// The command ran to its end, whatever its exit code.
    Exited,
    /// The run took longer than its timeout.
    TimedOut,
    /// Its principal cancelled it.
    Cancelled,
    /// The runner's executor failed before the command ended.
    ExecutorFailed,
}

/// One of a command's output streams, which the runner has uploaded as one object.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RunStream {
    /// The address of the stream's bytes.
    pub address: Address,
    /// Whether the runner cut the stream at its bound and kept only its end.
    pub cut: bool,
}

/// How many of a job's objects a runner took from the store and how many it already held.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ObjectCounts {
    /// Objects the runner fetched from the store.
    pub downloaded: u64,
    /// Objects the runner already held.
    pub from_cache: u64,
}

/// What a runner reports of a run, with every file it names already uploaded.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunReport {
    /// How the run ended.
    pub outcome: RunOutcome,
    /// The command's exit code; `None` unless it exited.
    pub exit_code: Option<i32>,
    /// The command's standard output; `None` where the command never started.
    pub stdout: Option<RunStream>,
    /// The command's standard error; `None` where the command never started.
    pub stderr: Option<RunStream>,
    /// The object counts.
    pub objects: ObjectCounts,
    /// The files the command left under the run's outputs.
    pub outputs: Vec<ProducedFile>,
    /// The files the command left under the run's cache paths.
    pub caches: Vec<ProducedFile>,
    /// What the command changed: each sent path whose content or kind differs or which is gone,
    /// and each path under `add` that exists.
    pub changes: BTreeMap<Path, Change>,
    /// The sent or `add` paths that hold anything but a regular file.
    pub refused: Vec<String>,
}

/// Why a run's changes were not written back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NotWritten {
    /// The run's instance was closed.
    InstanceClosed,
    /// The workspace's version was no longer the one the run started from.
    WorkspaceMoved,
    /// The principal may not write every changed path.
    WriteDenied,
    /// The run timed out.
    TimedOut,
    /// The run was cancelled.
    Cancelled,
    /// The executor failed.
    ExecutorFailed,
}

/// What became of a run's changes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WriteBack {
    /// The changes were applied to the workspace, or there were none.
    Written {
        /// The paths written; none where the run changed nothing.
        paths: Vec<Path>,
    },
    /// Nothing was written.
    NotWritten {
        /// Why.
        reason: NotWritten,
        /// The paths the workspace changed since the run started, where the reason is that it
        /// moved, or the paths the principal may not write; none for any other reason.
        paths: Vec<Path>,
    },
}

/// How a run ended, as the queue holds it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunResult {
    /// How the run ended.
    pub outcome: RunOutcome,
    /// The command's exit code.
    pub exit_code: Option<i32>,
    /// The command's standard output.
    pub stdout: Option<RunStream>,
    /// The command's standard error.
    pub stderr: Option<RunStream>,
    /// The object counts.
    pub objects: ObjectCounts,
    /// The address of the output tree; `None` where the run ended without a report.
    pub output_tree: Option<Address>,
    /// What the command changed.
    pub changes: BTreeMap<Path, Change>,
    /// The sent or `add` paths that hold anything but a regular file.
    pub refused: Vec<String>,
    /// What became of the changes.
    pub write_back: WriteBack,
}

impl RunResult {
    /// The result of a run that ended with no report from a runner, as `outcome` says.
    #[must_use]
    pub fn unreported(outcome: RunOutcome, reason: NotWritten) -> Self {
        Self {
            outcome,
            exit_code: None,
            stdout: None,
            stderr: None,
            objects: ObjectCounts::default(),
            output_tree: None,
            changes: BTreeMap::new(),
            refused: Vec::new(),
            write_back: WriteBack::NotWritten {
                reason,
                paths: Vec::new(),
            },
        }
    }
}

/// A run as the queue holds it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Run {
    /// Its identifier.
    pub id: RunId,
    /// Where it is in its life.
    pub state: RunState,
    /// What was asked.
    pub request: NewRun,
    /// The registration of the runner that holds it or held it last; `None` while it has not been
    /// claimed.
    pub runner: Option<RegistrationId>,
    /// How it ended, once a runner reported.
    pub result: Option<RunResult>,
}

impl Run {
    /// How the run ended, where it has: what the runner reported, or what the queue knows of a
    /// run it cancelled or timed out itself.
    #[must_use]
    pub fn ended(&self) -> Option<RunResult> {
        match self.state {
            RunState::Queued | RunState::Running { .. } => None,
            RunState::Cancelled => Some(RunResult::unreported(
                RunOutcome::Cancelled,
                NotWritten::Cancelled,
            )),
            RunState::TimedOut => Some(RunResult::unreported(
                RunOutcome::TimedOut,
                NotWritten::TimedOut,
            )),
            RunState::Finished => self.result.clone(),
        }
    }
}
