//! Repositories held by the in-memory adapters, with the check definitions read from TOML, with drafts staged and landed through the driving
//! ports.

use adapter_memory::{
    MemoryAuthorisationDecision, MemoryChangesetLog, MemoryDerivedData, MemoryDirectoryChanges,
    MemoryDraftStore, MemoryIdentityVerification, MemoryJobQueue, MemoryMembership,
    MemoryObjectStore, MemoryRepositoryDirectory, MemoryRunnerRegistry,
};
use adapter_toml::TomlCheckDefinitions;
use app::domain::check::CheckDefinition;
use app::port::driven::check_definitions::CheckFileError;
use test_support::contract::Deployment;
use test_support::contract::runner_deployment::RunnerDeployment;

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
    /// The runner registry.
    pub registry: MemoryRunnerRegistry,
    /// The job queue.
    pub queue: MemoryJobQueue,
    /// The check definitions.
    pub definitions: TomlCheckDefinitions,
}

impl Default for Harness {
    fn default() -> Self {
        let membership = MemoryMembership::default();
        Self {
            objects: MemoryObjectStore::default(),
            log: MemoryChangesetLog::default(),
            derived: MemoryDerivedData::default(),
            drafts: MemoryDraftStore::default(),
            directory: MemoryRepositoryDirectory::default(),
            identity: MemoryIdentityVerification::default(),
            authorisation: membership.authorisation_decision(),
            changes: MemoryDirectoryChanges::default(),
            registry: MemoryRunnerRegistry::default(),
            queue: MemoryJobQueue::default(),
            definitions: TomlCheckDefinitions::new().unwrap(),
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
}

impl RunnerDeployment for Harness {
    type Registry = MemoryRunnerRegistry;
    type Queue = MemoryJobQueue;
    type Definitions = TomlCheckDefinitions;

    fn registry(&self) -> &MemoryRunnerRegistry {
        &self.registry
    }

    fn queue(&self) -> &MemoryJobQueue {
        &self.queue
    }

    fn definitions(&self) -> &TomlCheckDefinitions {
        &self.definitions
    }

    fn teach(&self, _text: &str, _outcome: Result<Vec<CheckDefinition>, CheckFileError>) {
        // The reader parses the file itself, so there is nothing to teach it.
    }
}
