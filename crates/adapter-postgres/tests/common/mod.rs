//! Repositories held by the PostgreSQL adapters, each harness over a database of its own, with
//! drafts staged and landed through the driving ports.

pub mod proxy;

use std::sync::Arc;

use adapter_memory::{MemoryIdentityVerification, MemoryObjectStore};
use adapter_postgres::{
    Database, PostgresAuthorisationDecision, PostgresCacheRefs, PostgresChangesetLog,
    PostgresDerivedData, PostgresDirectoryOutbox, PostgresDraftStore, PostgresJobQueue,
    PostgresMembership, PostgresRepositoryDirectory, PostgresRunnerRegistry,
};
use adapter_toml::TomlCheckDefinitions;
use app::domain::check::CheckDefinition;
use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::repository::RepositoryId;
use app::domain::run::RunSettings;
use app::port::driven::check_definitions::CheckFileError;
use app::port::driven::directory_changes::{ChangeKind, RepositoryChange};
use sqlx::{Connection, PgConnection};
use test_support::TestDatabase;
use test_support::contract::Deployment;
use test_support::contract::runner_deployment::RunnerDeployment;

/// The adapters of one deployment, holding any number of repositories in a database the harness
/// created and drops with it. The object store and identity verification stay in memory.
pub struct Harness {
    /// Object storage, in memory.
    pub objects: Arc<MemoryObjectStore>,
    /// The changeset log.
    pub log: Arc<PostgresChangesetLog>,
    /// Derived data.
    pub derived: Arc<PostgresDerivedData>,
    /// Staged drafts.
    pub drafts: Arc<PostgresDraftStore>,
    /// The repository directory.
    pub directory: Arc<PostgresRepositoryDirectory>,
    /// The directory outbox.
    pub outbox: Arc<PostgresDirectoryOutbox>,
    /// Identity verification, in memory.
    pub identity: Arc<MemoryIdentityVerification>,
    /// Organisation membership and grants.
    pub membership: Arc<PostgresMembership>,
    /// The authorisation decision.
    pub authorisation: Arc<PostgresAuthorisationDecision>,
    /// The runner registry.
    pub registry: Arc<PostgresRunnerRegistry>,
    /// The job queue.
    pub queue: Arc<PostgresJobQueue>,
    /// The cache refs.
    pub cache_refs: Arc<PostgresCacheRefs>,
    /// The check definitions, read from TOML.
    pub definitions: Arc<TomlCheckDefinitions>,
    /// The directory changes reported: none.
    pub changes: Arc<()>,
    /// The delivery that follows a repository's creation: none.
    pub delivery: Arc<()>,
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
            objects: Arc::new(MemoryObjectStore::default()),
            log: Arc::new(PostgresChangesetLog::new(&database)),
            derived: Arc::new(PostgresDerivedData::new(&database)),
            drafts: Arc::new(PostgresDraftStore::new(&database)),
            directory: Arc::new(PostgresRepositoryDirectory::new(&database)),
            outbox: Arc::new(PostgresDirectoryOutbox::new(&database)),
            identity: Arc::new(MemoryIdentityVerification::default()),
            membership: Arc::new(PostgresMembership::new(&database)),
            authorisation: Arc::new(PostgresAuthorisationDecision::new(&database)),
            registry: Arc::new(PostgresRunnerRegistry::new(&database)),
            queue: Arc::new(PostgresJobQueue::new(&database)),
            cache_refs: Arc::new(PostgresCacheRefs::new(&database)),
            definitions: Arc::new(TomlCheckDefinitions::new().unwrap()),
            changes: Arc::new(()),
            delivery: Arc::new(()),
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
    type Outbox = PostgresDirectoryOutbox;
    type Delivery = ();
    type Registry = PostgresRunnerRegistry;
    type Queue = PostgresJobQueue;
    type CacheRefs = PostgresCacheRefs;
    type Definitions = TomlCheckDefinitions;

    async fn start() -> Self {
        Self::new().await
    }

    fn objects(&self) -> &Arc<MemoryObjectStore> {
        &self.objects
    }

    fn log(&self) -> &Arc<PostgresChangesetLog> {
        &self.log
    }

    fn derived(&self) -> &Arc<PostgresDerivedData> {
        &self.derived
    }

    fn drafts(&self) -> &Arc<PostgresDraftStore> {
        &self.drafts
    }

    fn directory(&self) -> &Arc<PostgresRepositoryDirectory> {
        &self.directory
    }

    fn identity(&self) -> &Arc<MemoryIdentityVerification> {
        &self.identity
    }

    fn membership(&self) -> &Arc<PostgresMembership> {
        &self.membership
    }

    fn authorisation(&self) -> &Arc<PostgresAuthorisationDecision> {
        &self.authorisation
    }

    fn changes(&self) -> &Arc<()> {
        &self.changes
    }

    fn outbox(&self) -> &Arc<PostgresDirectoryOutbox> {
        &self.outbox
    }

    fn delivery(&self) -> &Arc<()> {
        &self.delivery
    }

    fn registry(&self) -> &Arc<PostgresRunnerRegistry> {
        &self.registry
    }

    fn queue(&self) -> &Arc<PostgresJobQueue> {
        &self.queue
    }

    fn cache_refs(&self) -> &Arc<PostgresCacheRefs> {
        &self.cache_refs
    }

    fn definitions(&self) -> &Arc<TomlCheckDefinitions> {
        &self.definitions
    }

    async fn write_change(&self, repository: RepositoryId, kind: ChangeKind) -> RepositoryChange {
        let mut connection = PgConnection::connect(&self.url()).await.unwrap();
        let id = i64::try_from(repository.value()).unwrap();
        let text = match kind {
            ChangeKind::Created => "created",
            ChangeKind::Deleted => "deleted",
        };
        let (organisation, name, sequence): (String, String, i64) = sqlx::query_as(
            "with bumped as (update repositories set change_sequence = change_sequence + 1 \
                 where id = $1 returning organisation, name, change_sequence) \
             insert into directory_outbox (repository, organisation, name, kind, sequence) \
             select $1, organisation, name, $2, change_sequence from bumped \
             returning organisation, name, sequence",
        )
        .bind(id)
        .bind(text)
        .fetch_one(&mut connection)
        .await
        .unwrap();
        RepositoryChange {
            organisation: organisation.parse::<OrganisationName>().unwrap(),
            name: name.parse::<RepositoryName>().unwrap(),
            repository,
            kind,
            sequence: u64::try_from(sequence).unwrap(),
        }
    }
}

impl RunnerDeployment for Harness {
    fn teach(&self, _text: &str, _outcome: Result<Vec<CheckDefinition>, CheckFileError>) {
        // The reader parses the file itself, so there is nothing to teach it.
    }

    fn teach_run(&self, _text: &str, _outcome: Result<RunSettings, CheckFileError>) {
        // The reader parses the file itself, so there is nothing to teach it.
    }
}
