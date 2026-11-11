//! Repositories held by the PostgreSQL adapters, each harness over a database of its own, with
//! drafts staged and landed through the driving ports.

use adapter_memory::{MemoryIdentityVerification, MemoryObjectStore};
use adapter_postgres::{
    Database, PostgresAuthorisationDecision, PostgresChangesetLog, PostgresDerivedData,
    PostgresDraftStore, PostgresMembership, PostgresRepositoryDirectory,
};
use app::domain::address::Address;
use app::domain::changeset::Change;
use app::domain::credential::Credential;
use app::domain::file::{File, FileKind};
use app::domain::name::{DraftName, OrganisationName, RepositoryName};
use app::domain::path::Path;
use app::domain::principal::{Caller, Principal};
use app::domain::repository::RepositoryId;
use app::domain::tree::Entry;
use app::port::driven::changeset_log::{ChangesetLog, Ref};
use app::port::driven::derived_data::DerivedData;
use app::port::driven::object_store::ObjectStore;
use app::port::driving::authentication::Authentication;
use app::port::driving::repositories::Repositories;
use app::port::driving::source_control::{SourceControl, SourceControlError};
use app::use_case::authentication::AuthenticationService;
use app::use_case::land::Landing;
use app::use_case::repositories::RepositoriesService;
use app::use_case::source_control::SourceControlService;
use test_support::TestDatabase;

/// The largest upload the fixture's source control grants.
pub const MAX_UPLOAD_SIZE: u64 = 1024;

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
            database: test_database,
        }
    }

    /// The URL of this harness's database, for a pool of another making.
    pub fn url(&self) -> String {
        self.database.url()
    }
}

/// The source-control service over the harness's adapters.
pub type Service<'a> = SourceControlService<
    'a,
    MemoryObjectStore,
    PostgresChangesetLog,
    PostgresDerivedData,
    PostgresDraftStore,
    PostgresRepositoryDirectory,
    PostgresAuthorisationDecision,
>;

/// The repositories service over the harness's adapters.
pub type Repos<'a> = RepositoriesService<
    'a,
    PostgresRepositoryDirectory,
    PostgresAuthorisationDecision,
    PostgresMembership,
>;

impl Harness {
    /// The repositories port over this harness's adapters.
    pub const fn repositories(&self) -> Repos<'_> {
        RepositoriesService::new(&self.directory, &self.authorisation, &self.membership)
    }

    /// The authentication port over this harness's identity verification.
    pub const fn authentication(&self) -> AuthenticationService<'_, MemoryIdentityVerification> {
        AuthenticationService::new(&self.identity)
    }

    /// The principal `name` of the harness's issuer.
    ///
    /// # Panics
    ///
    /// Panics if a name is malformed.
    pub fn principal(name: &str) -> Principal {
        Principal::new("https://id.test", name).unwrap()
    }

    /// The credential the harness registers for `name`.
    ///
    /// # Panics
    ///
    /// Panics if a name is malformed.
    pub fn credential(name: &str) -> Credential {
        format!("token-{name}").parse().unwrap()
    }

    /// Registers a credential for the principal `name` and authenticates with it, through the
    /// authentication port.
    ///
    /// # Panics
    ///
    /// Panics if the credential is rejected.
    pub async fn caller(&self, name: &str) -> Caller {
        self.identity
            .register(&Self::credential(name), Self::principal(name));
        self.authentication()
            .authenticate(&Self::credential(name))
            .await
            .unwrap()
    }

    /// Source control over this harness's adapters.
    pub const fn source_control(&self) -> Service<'_> {
        SourceControlService::new(
            &self.objects,
            &self.log,
            &self.derived,
            &self.drafts,
            &self.directory,
            &self.authorisation,
            MAX_UPLOAD_SIZE,
        )
    }

    /// Creates the organisation `organisation`, which `owner` administers, holding the repository
    /// `app`, through the repositories port.
    ///
    /// # Panics
    ///
    /// Panics if a port call fails.
    pub async fn repository(&self, owner: &Caller, organisation: &str) -> Repo<'_> {
        let organisation: OrganisationName = organisation.parse().unwrap();
        let name: RepositoryName = "app".parse().unwrap();
        self.repositories()
            .create_organisation(owner, &organisation)
            .await
            .unwrap();
        let id = self
            .repositories()
            .create_repository(owner, &organisation, &name)
            .await
            .unwrap();
        Repo { harness: self, id }
    }
}

/// One repository of a harness.
pub struct Repo<'a> {
    /// The harness holding the repository.
    pub harness: &'a Harness,
    /// The repository's identifier.
    pub id: RepositoryId,
}

/// The draft name `name`.
///
/// # Panics
///
/// Panics if the name is malformed.
pub fn draft(name: &str) -> DraftName {
    name.parse().unwrap()
}

impl Repo<'_> {
    /// Stands in for the client uploading `content`: puts the bytes where an upload grant leads.
    ///
    /// # Panics
    ///
    /// Panics if the upload fails.
    pub async fn upload(&self, content: &str) -> Address {
        self.harness
            .objects
            .put(self.id, content.as_bytes().to_vec())
            .await
            .unwrap()
    }

    /// Uploads and stages each `(path, content)` as a regular file in `caller`'s draft `draft`.
    ///
    /// # Panics
    ///
    /// Panics if a path is malformed or a port call fails.
    pub async fn stage(&self, caller: &Caller, draft: &DraftName, files: &[(&str, &str)]) {
        for (path, content) in files {
            let content = self.upload(content).await;
            let file = File {
                kind: FileKind::Regular,
                content,
            };
            self.harness
                .source_control()
                .stage(
                    caller,
                    self.id,
                    draft,
                    path.parse().unwrap(),
                    Change::File(file),
                )
                .await
                .unwrap();
        }
    }

    /// Lands `caller`'s `draft` through the source-control port.
    ///
    /// # Errors
    ///
    /// Returns the source-control error the landing fails with.
    pub async fn land(
        &self,
        caller: &Caller,
        draft: &DraftName,
        message: &str,
    ) -> Result<Address, SourceControlError> {
        let landing = Landing {
            message: message.to_owned(),
            timestamp: 0,
        };
        self.harness
            .source_control()
            .land(caller, self.id, draft, landing)
            .await
    }

    /// Where `main` points.
    ///
    /// # Panics
    ///
    /// Panics if the read fails.
    pub async fn main(&self) -> Option<Address> {
        self.harness.log.read_ref(self.id, Ref::Main).await.unwrap()
    }

    /// The content of the file at `path` in `main`'s tree, if one is there.
    ///
    /// # Panics
    ///
    /// Panics if the path is malformed or a port call fails.
    pub async fn read(&self, path: &str) -> Option<String> {
        let path: Path = path.parse().unwrap();
        let derived = &self.harness.derived;
        let main = self.main().await?;
        let mut tree = derived.root(self.id, main).await.unwrap();
        let (name, directories) = path.components().split_last().unwrap();
        for directory in directories {
            match derived
                .tree(self.id, tree)
                .await
                .unwrap()
                .entries
                .get(directory)
            {
                Some(Entry::Directory(subtree)) => tree = *subtree,
                Some(Entry::File(_)) | None => return None,
            }
        }
        match derived.tree(self.id, tree).await.unwrap().entries.get(name) {
            Some(Entry::File(file)) => {
                let bytes = self
                    .harness
                    .objects
                    .get(self.id, file.content)
                    .await
                    .unwrap();
                Some(String::from_utf8(bytes).unwrap())
            }
            Some(Entry::Directory(_)) | None => None,
        }
    }
}
