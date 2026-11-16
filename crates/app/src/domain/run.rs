//! Runs: a command started in a workspace, queued as an unnamed check on it (ADR 0051).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::domain::address::Address;
use crate::domain::check::{Command, Timeout};
use crate::domain::draft::Instance;
use crate::domain::file::File;
use crate::domain::name::{DraftName, Label, OrganisationName};
use crate::domain::path::{Path, Prefix};
use crate::domain::principal::Principal;
use crate::domain::repository::RepositoryId;

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
    /// What `.codehub/run.toml` said when the run started.
    pub settings: RunSettings,
    /// The files sent: the workspace's tree under the prefix at `version`.
    pub files: BTreeMap<Path, File>,
    /// The tree of each cache path's workspace cache ref when the run started.
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
}

impl RunState {
    /// Whether the run has ended.
    #[must_use]
    pub const fn ended(self) -> bool {
        matches!(self, Self::Cancelled)
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
}
