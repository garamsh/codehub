//! Repositories held by the in-memory adapters, with drafts staged and landed through the driving
//! ports.

use adapter_memory::{
    MemoryAuthorisationDecision, MemoryChangesetLog, MemoryDerivedData, MemoryDirectoryChanges,
    MemoryDraftStore, MemoryIdentityVerification, MemoryMembership, MemoryObjectStore,
    MemoryRepositoryDirectory,
};
use test_support::contract::Deployment;

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
