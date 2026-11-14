//! The adapters one served API runs over: the in-memory ones, and an object store of the caller's
//! choice.

use adapter_memory::{
    MemoryAuthorisationDecision, MemoryChangesetLog, MemoryCheckDefinitions, MemoryDerivedData,
    MemoryDraftStore, MemoryIdentityVerification, MemoryJobQueue, MemoryMembership,
    MemoryRepositoryDirectory, MemoryRunnerRegistry,
};
use app::use_case::administration::Administration;
use app::use_case::assembly::Assembly;
use app::use_case::landing_attempts::LandingAttempts;

/// How long a claim or a heartbeat holds a job, in seconds.
const LEASE: i64 = 60;

/// The adapters of one deployment: `objects` of the caller's choice, and the others in memory.
pub struct Stores<O> {
    /// The object store.
    pub objects: O,
    /// The changeset log.
    pub log: MemoryChangesetLog,
    /// The derived data.
    pub derived: MemoryDerivedData,
    /// The draft store.
    pub drafts: MemoryDraftStore,
    /// The repository directory.
    pub directory: MemoryRepositoryDirectory,
    /// Identity verification, where tokens are registered.
    pub identity: MemoryIdentityVerification,
    /// Membership.
    pub membership: MemoryMembership,
    /// The authorisation decision over `membership`.
    pub authorisation: MemoryAuthorisationDecision,
    /// The runner registry.
    pub registry: MemoryRunnerRegistry,
    /// The job queue.
    pub queue: MemoryJobQueue,
    /// The check definitions.
    pub definitions: MemoryCheckDefinitions,
}

/// The driving ports over the adapters of [`Stores`].
pub type StoresAssembly<'a, O, I> = Assembly<
    'a,
    O,
    MemoryChangesetLog,
    MemoryDerivedData,
    MemoryDraftStore,
    MemoryRepositoryDirectory,
    MemoryAuthorisationDecision,
    MemoryMembership,
    MemoryJobQueue,
    MemoryRunnerRegistry,
    MemoryCheckDefinitions,
    (),
    (),
    I,
>;

impl<O: 'static> Stores<O> {
    /// Fresh adapters around `objects`, which live as long as the process: the services borrow
    /// them, and a router holds its ports for `'static`.
    ///
    /// # Panics
    ///
    /// Panics if the check file path is not valid, which it always is.
    #[must_use]
    pub fn leaked(objects: O) -> &'static Self {
        let membership = MemoryMembership::default();
        Box::leak(Box::new(Self {
            objects,
            log: MemoryChangesetLog::default(),
            derived: MemoryDerivedData::default(),
            drafts: MemoryDraftStore::default(),
            directory: MemoryRepositoryDirectory::default(),
            identity: MemoryIdentityVerification::default(),
            authorisation: membership.authorisation_decision(),
            membership,
            registry: MemoryRunnerRegistry::default(),
            queue: MemoryJobQueue::default(),
            definitions: MemoryCheckDefinitions::new(".codehub/checks.toml".parse().unwrap()),
        }))
    }
}

impl<O> Stores<O> {
    /// The driving ports over these adapters, verifying credentials with `identity`, with members
    /// administered as `administration` says and uploads up to `max_upload_size` bytes.
    #[must_use]
    pub fn assembly<'a, I>(
        &'a self,
        identity: &'a I,
        administration: Administration,
        max_upload_size: u64,
    ) -> StoresAssembly<'a, O, I> {
        Assembly {
            objects: &self.objects,
            log: &self.log,
            derived: &self.derived,
            drafts: &self.drafts,
            directory: &self.directory,
            authorisation: &self.authorisation,
            membership: &self.membership,
            queue: &self.queue,
            registry: &self.registry,
            definitions: &self.definitions,
            changes: &(),
            delivery: &(),
            identity,

            administration,
            max_upload_size,
            attempts: LandingAttempts::default(),
            lease: LEASE,
        }
    }
}
