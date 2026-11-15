//! Repositories held by the in-memory adapters, with drafts staged and landed through the driving
//! ports, over the check definitions a [`Definitions`] names.

use std::sync::Arc;

use adapter_memory::{
    MemoryAuthorisationDecision, MemoryCacheRefs, MemoryChangesetLog, MemoryCheckDefinitions,
    MemoryDerivedData, MemoryDirectoryChanges, MemoryDirectoryOutbox, MemoryDraftStore,
    MemoryIdentityVerification, MemoryJobQueue, MemoryMembership, MemoryObjectStore,
    MemoryRepositoryDirectory, MemoryRunnerRegistry,
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
    type Adapter: CheckDefinitions + 'static;

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
    pub objects: Arc<MemoryObjectStore>,
    /// The changeset log.
    pub log: Arc<MemoryChangesetLog>,
    /// The derived data.
    pub derived: Arc<MemoryDerivedData>,
    /// The draft store.
    pub drafts: Arc<MemoryDraftStore>,
    /// The repository directory.
    pub directory: Arc<MemoryRepositoryDirectory>,
    /// The identity verification.
    pub identity: Arc<MemoryIdentityVerification>,
    /// The membership.
    pub membership: Arc<MemoryMembership>,
    /// The authorisation decision, derived from the membership.
    pub authorisation: Arc<MemoryAuthorisationDecision>,
    /// The directory changes reported.
    pub changes: Arc<MemoryDirectoryChanges>,
    /// The directory outbox, over the entries the repository directory writes.
    pub outbox: Arc<MemoryDirectoryOutbox>,
    /// The relay that offers each creation's entry to a receiver refusing it.
    pub delivery: Arc<HeldRelay>,
    /// The runner registry.
    pub registry: Arc<MemoryRunnerRegistry>,
    /// The job queue.
    pub queue: Arc<MemoryJobQueue>,
    /// The cache refs.
    pub cache_refs: Arc<MemoryCacheRefs>,
    /// The check definitions.
    pub definitions: Arc<K::Adapter>,
}

impl<K: Definitions> Default for MemoryDeployment<K> {
    fn default() -> Self {
        let membership = MemoryMembership::default();
        let directory = MemoryRepositoryDirectory::default();
        Self {
            objects: Arc::default(),
            log: Arc::default(),
            derived: Arc::default(),
            drafts: Arc::default(),
            outbox: Arc::new(directory.outbox()),
            delivery: Arc::new(HeldRelay::new(directory.outbox())),
            directory: Arc::new(directory),
            identity: Arc::default(),
            authorisation: Arc::new(membership.authorisation_decision()),
            changes: Arc::default(),
            registry: Arc::default(),
            queue: Arc::default(),
            cache_refs: Arc::default(),
            definitions: Arc::new(K::start()),
            membership: Arc::new(membership),
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

    fn objects(&self) -> &Arc<MemoryObjectStore> {
        &self.objects
    }

    fn log(&self) -> &Arc<MemoryChangesetLog> {
        &self.log
    }

    fn derived(&self) -> &Arc<MemoryDerivedData> {
        &self.derived
    }

    fn drafts(&self) -> &Arc<MemoryDraftStore> {
        &self.drafts
    }

    fn directory(&self) -> &Arc<MemoryRepositoryDirectory> {
        &self.directory
    }

    fn identity(&self) -> &Arc<MemoryIdentityVerification> {
        &self.identity
    }

    fn membership(&self) -> &Arc<MemoryMembership> {
        &self.membership
    }

    fn authorisation(&self) -> &Arc<MemoryAuthorisationDecision> {
        &self.authorisation
    }

    fn changes(&self) -> &Arc<MemoryDirectoryChanges> {
        &self.changes
    }

    fn outbox(&self) -> &Arc<MemoryDirectoryOutbox> {
        &self.outbox
    }

    fn delivery(&self) -> &Arc<HeldRelay> {
        &self.delivery
    }

    async fn write_change(&self, repository: RepositoryId, kind: ChangeKind) -> RepositoryChange {
        self.directory.record_change(repository, kind).unwrap()
    }
}

impl<K: Definitions> RunnerDeployment for MemoryDeployment<K> {
    type Registry = MemoryRunnerRegistry;
    type Queue = MemoryJobQueue;
    type CacheRefs = MemoryCacheRefs;
    type Definitions = K::Adapter;

    fn registry(&self) -> &Arc<MemoryRunnerRegistry> {
        &self.registry
    }

    fn queue(&self) -> &Arc<MemoryJobQueue> {
        &self.queue
    }

    fn cache_refs(&self) -> &Arc<MemoryCacheRefs> {
        &self.cache_refs
    }

    fn definitions(&self) -> &Arc<K::Adapter> {
        &self.definitions
    }

    fn teach(&self, text: &str, outcome: Result<Vec<CheckDefinition>, CheckFileError>) {
        K::teach(&self.definitions, text, outcome);
    }
}
