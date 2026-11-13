//! Repositories held by the in-memory adapters, with drafts staged and landed through the driving
//! ports.

use adapter_memory::{
    MemoryAuthorisationDecision, MemoryChangesetLog, MemoryCheckDefinitions, MemoryDerivedData,
    MemoryDirectoryChanges, MemoryDirectoryOutbox, MemoryDraftStore, MemoryIdentityVerification,
    MemoryJobQueue, MemoryMembership, MemoryObjectStore, MemoryRepositoryDirectory,
    MemoryRunnerRegistry,
};
use app::domain::check::CheckDefinition;
use app::domain::repository::RepositoryId;
use app::port::driven::check_definitions::CheckFileError;
use app::port::driven::directory_changes::{ChangeKind, RepositoryChange};
use test_support::contract::runner_deployment::RunnerDeployment;
use test_support::contract::{Deployment, HeldRelay};

/// The in-memory adapters of one deployment, holding any number of repositories.
pub struct Harness {
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
    pub definitions: MemoryCheckDefinitions,
}

impl Default for Harness {
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
            definitions: MemoryCheckDefinitions::new(".codehub/checks".parse().unwrap()),
            membership,
        }
    }
}

impl Deployment for Harness {
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

impl RunnerDeployment for Harness {
    type Registry = MemoryRunnerRegistry;
    type Queue = MemoryJobQueue;
    type Definitions = MemoryCheckDefinitions;

    fn registry(&self) -> &MemoryRunnerRegistry {
        &self.registry
    }

    fn queue(&self) -> &MemoryJobQueue {
        &self.queue
    }

    fn definitions(&self) -> &MemoryCheckDefinitions {
        &self.definitions
    }

    fn teach(&self, text: &str, outcome: Result<Vec<CheckDefinition>, CheckFileError>) {
        self.definitions.teach(text, outcome);
    }
}
