//! Repositories held by the PostgreSQL adapters, each harness over a database of its own, with
//! drafts staged and landed through the driving ports.

use adapter_memory::{MemoryIdentityVerification, MemoryObjectStore};
use adapter_postgres::{
    Database, PostgresAuthorisationDecision, PostgresChangesetLog, PostgresDerivedData,
    PostgresDraftStore, PostgresJobQueue, PostgresMembership, PostgresRepositoryDirectory,
    PostgresRunnerRegistry,
};
use adapter_toml::TomlCheckDefinitions;
use app::domain::check::CheckDefinition;
use app::port::driven::check_definitions::CheckFileError;
use test_support::TestDatabase;
use test_support::contract::Deployment;
use test_support::contract::runner_deployment::RunnerDeployment;

/// The adapters of one deployment, holding any number of repositories in a database the harness
/// created and drops with it. The object store and identity verification stay in memory.
pub struct Harness {
    /// Object storage, in memory.
    pub objects: MemoryObjectStore,
    /// The changeset log.
    pub log: PostgresChangesetLog,
    /// Derived data.
    pub derived: PostgresDerivedData,
    /// Staged drafts.
    pub drafts: PostgresDraftStore,
    /// The repository directory.
    pub directory: PostgresRepositoryDirectory,
    /// Identity verification, in memory.
    pub identity: MemoryIdentityVerification,
    /// Organisation membership and grants.
    pub membership: PostgresMembership,
    /// The authorisation decision.
    pub authorisation: PostgresAuthorisationDecision,
    /// The runner registry.
    pub registry: PostgresRunnerRegistry,
    /// The job queue.
    pub queue: PostgresJobQueue,
    /// The check definitions, read from TOML.
    pub definitions: TomlCheckDefinitions,
    database: TestDatabase,
}

impl Harness {
    /// Creates a database and migrates it, with a pool of three connections.
    pub async fn new() -> Self {
        Self::with_pool(3).await
    }

    /// As [`Harness::new`], with a pool of `pool_size` connections.
    ///
    /// # Panics
    ///
    /// Panics if the server cannot be reached or the database cannot be created or migrated.
    pub async fn with_pool(pool_size: u32) -> Self {
        let test_database = TestDatabase::new().await;
        let database = Database::connect(&test_database.url(), pool_size)
            .await
            .unwrap();
        database.migrate().await.unwrap();
        Self {
            objects: MemoryObjectStore::default(),
            log: PostgresChangesetLog::new(&database),
            derived: PostgresDerivedData::new(&database),
            drafts: PostgresDraftStore::new(&database),
            directory: PostgresRepositoryDirectory::new(&database),
            identity: MemoryIdentityVerification::default(),
            membership: PostgresMembership::new(&database),
            authorisation: PostgresAuthorisationDecision::new(&database),
            registry: PostgresRunnerRegistry::new(&database),
            queue: PostgresJobQueue::new(&database),
            definitions: TomlCheckDefinitions::new().unwrap(),
            database: test_database,
        }
    }

    /// The URL of this harness's database, for a pool of another making.
    pub fn url(&self) -> String {
        self.database.url()
    }
}

impl Deployment for Harness {
    type Log = PostgresChangesetLog;
    type Derived = PostgresDerivedData;
    type Drafts = PostgresDraftStore;
    type Directory = PostgresRepositoryDirectory;
    type Membership = PostgresMembership;
    type Authorisation = PostgresAuthorisationDecision;
    type Changes = ();

    async fn start() -> Self {
        Self::new().await
    }

    fn objects(&self) -> &MemoryObjectStore {
        &self.objects
    }

    fn log(&self) -> &PostgresChangesetLog {
        &self.log
    }

    fn derived(&self) -> &PostgresDerivedData {
        &self.derived
    }

    fn drafts(&self) -> &PostgresDraftStore {
        &self.drafts
    }

    fn directory(&self) -> &PostgresRepositoryDirectory {
        &self.directory
    }

    fn identity(&self) -> &MemoryIdentityVerification {
        &self.identity
    }

    fn membership(&self) -> &PostgresMembership {
        &self.membership
    }

    fn authorisation(&self) -> &PostgresAuthorisationDecision {
        &self.authorisation
    }

    fn changes(&self) -> &() {
        &()
    }
}

impl RunnerDeployment for Harness {
    type Registry = PostgresRunnerRegistry;
    type Queue = PostgresJobQueue;
    type Definitions = TomlCheckDefinitions;

    fn registry(&self) -> &PostgresRunnerRegistry {
        &self.registry
    }

    fn queue(&self) -> &PostgresJobQueue {
        &self.queue
    }

    fn definitions(&self) -> &TomlCheckDefinitions {
        &self.definitions
    }

    fn teach(&self, _text: &str, _outcome: Result<Vec<CheckDefinition>, CheckFileError>) {
        // The reader parses the file itself, so there is nothing to teach it.
    }
}
