//! The adapters one served API runs over: the in-memory ones, and an object store of the caller's
//! choice.

use std::sync::Arc;

use adapter_memory::{
    MemoryAuthorisationDecision, MemoryCacheRefs, MemoryChangesetLog, MemoryCheckDefinitions,
    MemoryDerivedData, MemoryDraftStore, MemoryIdentityVerification, MemoryJobQueue,
    MemoryMembership, MemoryRepositoryDirectory, MemoryRunnerRegistry,
};
use app::port::driven::identity_verification::IdentityVerification;
use app::port::driven::object_store::ObjectStore;
use app::use_case::administration::Administration;
use app::use_case::assembly::Assembly;
use app::use_case::landing_attempts::LandingAttempts;

/// How long a claim or a heartbeat holds a job, in seconds.
const LEASE: i64 = 60;

/// The adapters of one deployment: `objects` of the caller's choice, and the others in memory.
pub struct Stores<O> {
    /// The object store.
    pub objects: Arc<O>,
    /// The changeset log.
    pub log: Arc<MemoryChangesetLog>,
    /// The derived data.
    pub derived: Arc<MemoryDerivedData>,
    /// The draft store.
    pub drafts: Arc<MemoryDraftStore>,
    /// The repository directory.
    pub directory: Arc<MemoryRepositoryDirectory>,
    /// Identity verification, where tokens are registered.
    pub identity: Arc<MemoryIdentityVerification>,
    /// Membership.
    pub membership: Arc<MemoryMembership>,
    /// The authorisation decision over `membership`.
    pub authorisation: Arc<MemoryAuthorisationDecision>,
    /// The runner registry.
    pub registry: Arc<MemoryRunnerRegistry>,
    /// The job queue.
    pub queue: Arc<MemoryJobQueue>,
    /// The cache refs.
    pub cache_refs: Arc<MemoryCacheRefs>,
    /// The check definitions.
    pub definitions: Arc<MemoryCheckDefinitions>,
}

impl<O: ObjectStore + 'static> Stores<O> {
    /// Fresh adapters around `objects`.
    ///
    /// # Panics
    ///
    /// Panics if the check file path is not valid, which it always is.
    #[must_use]
    pub fn new(objects: O) -> Self {
        let membership = MemoryMembership::default();
        Self {
            objects: Arc::new(objects),
            log: Arc::default(),
            derived: Arc::default(),
            drafts: Arc::default(),
            directory: Arc::default(),
            identity: Arc::default(),
            authorisation: Arc::new(membership.authorisation_decision()),
            membership: Arc::new(membership),
            registry: Arc::default(),
            queue: Arc::default(),
            cache_refs: Arc::default(),
            definitions: Arc::new(MemoryCheckDefinitions::new(
                ".codehub/checks.toml".parse().unwrap(),
            )),
        }
    }

    /// The driving ports over these adapters, verifying credentials with `identity`, with members
    /// administered as `administration` says and uploads up to `max_upload_size` bytes.
    #[must_use]
    pub fn assembly(
        &self,
        identity: Arc<dyn IdentityVerification>,
        administration: Administration,
        max_upload_size: u64,
    ) -> Assembly {
        Assembly {
            objects: self.objects.clone(),
            log: self.log.clone(),
            derived: self.derived.clone(),
            drafts: self.drafts.clone(),
            directory: self.directory.clone(),
            authorisation: self.authorisation.clone(),
            membership: self.membership.clone(),
            queue: self.queue.clone(),
            cache_refs: self.cache_refs.clone(),
            registry: self.registry.clone(),
            definitions: self.definitions.clone(),
            changes: Arc::new(()),
            delivery: Arc::new(()),
            identity,

            administration,
            max_upload_size,
            attempts: LandingAttempts::default(),
            lease: LEASE,
        }
    }
}
