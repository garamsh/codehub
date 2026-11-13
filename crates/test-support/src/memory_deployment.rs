//! Repositories held by the in-memory adapters, with drafts staged and landed through the driving
//! ports, over the check definitions a [`Definitions`] names.

use adapter_memory::{
    MemoryAuthorisationDecision, MemoryChangesetLog, MemoryCheckDefinitions, MemoryDerivedData,
    MemoryDirectoryChanges, MemoryDirectoryOutbox, MemoryDraftStore, MemoryIdentityVerification,
    MemoryJobQueue, MemoryMembership, MemoryObjectStore, MemoryRepositoryDirectory,
    MemoryRunnerRegistry,
};
use app::domain::check::CheckDefinition;
use app::domain::repository::RepositoryId;
use app::port::driven::check_definitions::{CheckDefinitions, CheckFileError};
use app::port::driven::directory_changes::{ChangeKind, RepositoryChange};

use crate::contract::runner_deployment::RunnerDeployment;
use crate::contract::{Deployment, HeldRelay};

/// The check definitions a [`MemoryDeployment`] holds: the adapter, how a fresh one starts, and how
/// it is made to read a file as a given outcome.
///
/// A marker type names them, so that a package can name the adapter of its own technology without
/// this library depending on it.
pub trait Definitions {
    /// The adapter.
    type Adapter: CheckDefinitions + Send + Sync;

    /// A fresh adapter.
    fn start() -> Self::Adapter;

    /// Makes `adapter` read the file content `text` as `outcome`. An adapter that parses the file
    /// format itself ignores `outcome`, so `text` is always in that format and `outcome` is what
    /// it reads as.
    fn teach(
        adapter: &Self::Adapter,
        text: &str,
        outcome: Result<Vec<CheckDefinition>, CheckFileError>,
    );
}

/// The in-memory check definitions, which read a file as whatever they are taught.
pub struct Taught;

impl Definitions for Taught {
    type Adapter = MemoryCheckDefinitions;

    fn start() -> MemoryCheckDefinitions {
        MemoryCheckDefinitions::new(".codehub/checks".parse().unwrap())
    }

    fn teach(
        adapter: &MemoryCheckDefinitions,
        text: &str,
        outcome: Result<Vec<CheckDefinition>, CheckFileError>,
    ) {
        adapter.teach(text, outcome);
    }
}

/// The in-memory adapters of one deployment, holding any number of repositories.
pub struct MemoryDeployment<K: Definitions> {
    /// The object store.
    pub objects: MemoryObjectStore,
    /// The changeset log.
    pub log: MemoryChangesetLog,
    /// The derived data.
    pub derived: MemoryDerivedData,
    /// The draft store.
    pub drafts: MemoryDraftStore,
    /// The repository directory.
    pub directory: MemoryRepositoryDirectory,
    /// The identity verification.
    pub identity: MemoryIdentityVerification,
    /// The membership.
    pub membership: MemoryMembership,
    /// The authorisation decision, derived from the membership.
    pub authorisation: MemoryAuthorisationDecision,
    /// The directory changes reported.
    pub changes: MemoryDirectoryChanges,
    /// The directory outbox, over the entries the repository directory writes.
    pub outbox: MemoryDirectoryOutbox,
    /// The relay that offers each creation's entry to a receiver refusing it.
    pub delivery: HeldRelay,
    /// The runner registry.
    pub registry: MemoryRunnerRegistry,
    /// The job queue.
    pub queue: MemoryJobQueue,
    /// The check definitions.
    pub definitions: K::Adapter,
}

impl<K: Definitions> Default for MemoryDeployment<K> {
    fn default() -> Self {
        let membership = MemoryMembership::default();
        let directory = MemoryRepositoryDirectory::default();
        Self {
            objects: MemoryObjectStore::default(),
            log: MemoryChangesetLog::default(),
            derived: MemoryDerivedData::default(),
            drafts: MemoryDraftStore::default(),
            outbox: directory.outbox(),
            delivery: HeldRelay::new(directory.outbox()),
            directory,
            identity: MemoryIdentityVerification::default(),
            authorisation: membership.authorisation_decision(),
            changes: MemoryDirectoryChanges::default(),
            registry: MemoryRunnerRegistry::default(),
            queue: MemoryJobQueue::default(),
            definitions: K::start(),
            membership,
        }
    }
}

impl<K: Definitions> Deployment for MemoryDeployment<K> {
    type Log = MemoryChangesetLog;
    type Derived = MemoryDerivedData;
    type Drafts = MemoryDraftStore;
    type Directory = MemoryRepositoryDirectory;
    type Membership = MemoryMembership;
    type Authorisation = MemoryAuthorisationDecision;
    type Changes = MemoryDirectoryChanges;
    type Outbox = MemoryDirectoryOutbox;
    type Delivery = HeldRelay;

    async fn start() -> Self {
        Self::default()
    }

    fn objects(&self) -> &MemoryObjectStore {
        &self.objects
    }

    fn log(&self) -> &MemoryChangesetLog {
        &self.log
    }

    fn derived(&self) -> &MemoryDerivedData {
        &self.derived
    }

    fn drafts(&self) -> &MemoryDraftStore {
        &self.drafts
    }

    fn directory(&self) -> &MemoryRepositoryDirectory {
        &self.directory
    }

    fn identity(&self) -> &MemoryIdentityVerification {
        &self.identity
    }

    fn membership(&self) -> &MemoryMembership {
        &self.membership
    }

    fn authorisation(&self) -> &MemoryAuthorisationDecision {
        &self.authorisation
    }

    fn changes(&self) -> &MemoryDirectoryChanges {
        &self.changes
    }

    fn outbox(&self) -> &MemoryDirectoryOutbox {
        &self.outbox
    }

    fn delivery(&self) -> &HeldRelay {
        &self.delivery
    }

    async fn write_change(&self, repository: RepositoryId, kind: ChangeKind) -> RepositoryChange {
        self.directory.record_change(repository, kind).unwrap()
    }
}

impl<K: Definitions> RunnerDeployment for MemoryDeployment<K> {
    type Registry = MemoryRunnerRegistry;
    type Queue = MemoryJobQueue;
    type Definitions = K::Adapter;

    fn registry(&self) -> &MemoryRunnerRegistry {
        &self.registry
    }

    fn queue(&self) -> &MemoryJobQueue {
        &self.queue
    }

    fn definitions(&self) -> &K::Adapter {
        &self.definitions
    }

    fn teach(&self, text: &str, outcome: Result<Vec<CheckDefinition>, CheckFileError>) {
        K::teach(&self.definitions, text, outcome);
    }
}
