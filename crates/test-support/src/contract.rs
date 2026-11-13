//! The contract scenarios that every adapter set of the driven ports must pass, written once
//! against [`Deployment`]; each package that holds an adapter set implements it and lists the
//! scenarios as its tests.

#![expect(
    clippy::missing_panics_doc,
    reason = "every scenario and helper panics as a failed assertion does"
)]

pub mod access;
pub mod checks;
pub mod history;
pub mod landing;
pub mod object_store;
pub mod outbox;
pub mod reverts;
pub mod runner_deployment;
pub mod runners;
pub mod scoping;
pub mod workspace;

use std::future::Future;

use adapter_memory::{MemoryIdentityVerification, MemoryObjectStore};
use app::domain::address::Address;
use app::domain::changeset::Change;
use app::domain::credential::Credential;
use app::domain::file::{File, FileKind};
use app::domain::name::{DraftName, OrganisationName, RepositoryName};
use app::domain::path::Path;
use app::domain::principal::{Caller, Principal};
use app::domain::repository::RepositoryId;
use app::domain::tree::Entry;
use app::port::driven::authorisation_decision::AuthorisationDecision;
use app::port::driven::changeset_log::{ChangesetLog, Ref};
use app::port::driven::derived_data::DerivedData;
use app::port::driven::directory_changes::{ChangeKind, DirectoryChanges, RepositoryChange};
use app::port::driven::directory_outbox::DirectoryOutbox;
use app::port::driven::draft_store::DraftStore;
use app::port::driven::membership::Membership;
use app::port::driven::object_store::ObjectStore;
use app::port::driven::repository_directory::RepositoryDirectory;
use app::port::driving::authentication::Authentication;
use app::port::driving::repositories::Repositories;
use app::port::driving::source_control::Landing;
use app::port::driving::source_control::{SourceControl, SourceControlError};
use app::use_case::administration::Administration;
use app::use_case::authentication::AuthenticationService;
use app::use_case::relay::{Delivery, DirectoryRelay, RelayEnvironment};
use app::use_case::repositories::{RepositoriesAdapters, RepositoriesService};
use app::use_case::source_control::{SourceControlAdapters, SourceControlService};

/// The largest upload the deployment's source control grants.
pub const MAX_UPLOAD_SIZE: u64 = 1024;

/// The repositories service over a deployment's adapters.
pub type Repos<'a, H> = RepositoriesService<
    'a,
    <H as Deployment>::Directory,
    <H as Deployment>::Authorisation,
    <H as Deployment>::Membership,
    <H as Deployment>::Changes,
    <H as Deployment>::Delivery,
>;

/// The source-control service over a deployment's adapters.
pub type Service<'a, H> = SourceControlService<
    'a,
    MemoryObjectStore,
    <H as Deployment>::Log,
    <H as Deployment>::Derived,
    <H as Deployment>::Drafts,
    <H as Deployment>::Directory,
    <H as Deployment>::Authorisation,
>;

/// The adapters of one deployment, holding any number of repositories. Object storage and
/// identity verification are in memory in every deployment.
pub trait Deployment: Sized + Send + Sync {
    /// The changeset log.
    type Log: ChangesetLog + Sync;
    /// The derived data.
    type Derived: DerivedData + Sync;
    /// The draft store.
    type Drafts: DraftStore + Sync;
    /// The repository directory.
    type Directory: RepositoryDirectory + Sync;
    /// The organisation membership.
    type Membership: Membership + Sync;
    /// The authorisation decision, over the membership.
    type Authorisation: AuthorisationDecision + Sync;
    /// The directory changes reported.
    type Changes: DirectoryChanges + Sync;
    /// The directory outbox.
    type Outbox: DirectoryOutbox + Sync;
    /// The delivery that follows a repository's creation.
    type Delivery: Delivery + Sync;

    /// A deployment of its own, with no repository.
    fn start() -> impl Future<Output = Self> + Send;

    /// The object store.
    fn objects(&self) -> &MemoryObjectStore;
    /// The changeset log.
    fn log(&self) -> &Self::Log;
    /// The derived data.
    fn derived(&self) -> &Self::Derived;
    /// The draft store.
    fn drafts(&self) -> &Self::Drafts;
    /// The repository directory.
    fn directory(&self) -> &Self::Directory;
    /// The identity verification.
    fn identity(&self) -> &MemoryIdentityVerification;
    /// The membership.
    fn membership(&self) -> &Self::Membership;
    /// The authorisation decision.
    fn authorisation(&self) -> &Self::Authorisation;
    /// The directory changes reported.
    fn changes(&self) -> &Self::Changes;
    /// The directory outbox, over the entries the repository directory writes.
    fn outbox(&self) -> &Self::Outbox;
    /// The delivery of a repository's outbox entries to the directory changes.
    fn delivery(&self) -> &Self::Delivery;

    /// Writes the entry of a further change to `repository`, as the repository directory does for
    /// a deletion once repositories can be deleted, and returns it.
    fn write_change(
        &self,
        repository: RepositoryId,
        kind: ChangeKind,
    ) -> impl Future<Output = RepositoryChange> + Send;

    /// The repositories port over this deployment's adapters, administered by codehub.
    fn repositories(&self) -> Repos<'_, Self> {
        self.administered(Administration::BuiltIn)
    }

    /// The repositories port over this deployment's adapters, with members administered as
    /// `administration` says.
    fn administered(&self, administration: Administration) -> Repos<'_, Self> {
        RepositoriesService::new(
            RepositoriesAdapters {
                directory: self.directory(),
                authorisation: self.authorisation(),
                membership: self.membership(),
                changes: self.changes(),
                delivery: self.delivery(),
            },
            administration,
        )
    }

    /// The authentication port over this deployment's identity verification.
    fn authentication(&self) -> AuthenticationService<'_, MemoryIdentityVerification> {
        AuthenticationService::new(self.identity())
    }

    /// Source control over this deployment's adapters.
    fn source_control(&self) -> Service<'_, Self> {
        SourceControlService::new(
            SourceControlAdapters {
                objects: self.objects(),
                log: self.log(),
                derived: self.derived(),
                drafts: self.drafts(),
                directory: self.directory(),
                authorisation: self.authorisation(),
            },
            MAX_UPLOAD_SIZE,
        )
    }

    /// The principal `name` of the deployment's issuer.
    fn principal(name: &str) -> Principal {
        Principal::new("https://id.test", name).unwrap()
    }

    /// The credential the deployment registers for `name`.
    fn credential(name: &str) -> Credential {
        format!("token-{name}").parse().unwrap()
    }

    /// Registers a credential for the principal `name` and authenticates with it, through the
    /// authentication port.
    fn caller(&self, name: &str) -> impl Future<Output = Caller> + Send {
        async move {
            self.identity()
                .register(&Self::credential(name), Self::principal(name));
            self.authentication()
                .authenticate(&Self::credential(name))
                .await
                .unwrap()
        }
    }

    /// Creates the organisation `organisation`, which `owner` administers, holding the repository
    /// `app`, through the repositories port.
    fn repository(
        &self,
        owner: &Caller,
        organisation: &str,
    ) -> impl Future<Output = Repo<'_, Self>> + Send {
        async move {
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
}

/// One repository of a deployment.
pub struct Repo<'a, H> {
    /// The deployment holding the repository.
    pub harness: &'a H,
    /// The repository's identifier.
    pub id: RepositoryId,
}

/// The environment of a relay that runs in the real time of the test.
pub struct RealTime;

impl RelayEnvironment for RealTime {
    fn now(&self) -> i64 {
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        i64::try_from(since.as_secs()).unwrap()
    }

    fn sleep(&self, duration: std::time::Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(duration)
    }

    fn log(&self, line: &str) {
        eprintln!("{line}");
    }
}

/// A receiver that refuses every repository change, and counts the attempts.
#[derive(Default)]
pub struct Holding {
    attempts: std::sync::atomic::AtomicUsize,
}

impl Holding {
    /// How many repository changes were offered to it.
    pub fn attempts(&self) -> usize {
        self.attempts.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl DirectoryChanges for Holding {
    async fn repository_changed(&self, _change: &RepositoryChange) -> bool {
        self.attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        false
    }

    async fn organisation_changed(
        &self,
        _change: &app::port::driven::directory_changes::OrganisationChange,
    ) {
    }
}

/// The environment of a relay in a test that reads the outbox afterwards: a clock at the start of
/// time, so that what a failed delivery holds back is claimable at any time a test names, and
/// sleeps that never end.
pub struct Frozen;

impl RelayEnvironment for Frozen {
    fn now(&self) -> i64 {
        i64::MIN
    }

    fn sleep(&self, _duration: std::time::Duration) -> impl Future<Output = ()> + Send {
        std::future::pending()
    }

    fn log(&self, _line: &str) {}
}

/// The relay of an in-memory deployment: it takes each creation's entry from the outbox and offers
/// it to a receiver that refuses it, so that the entry stays for the test to read.
pub struct HeldRelay {
    /// The receiver the relay delivers to.
    pub receiver: std::sync::Arc<Holding>,
    relay: DirectoryRelay<adapter_memory::MemoryDirectoryOutbox, Holding, Frozen>,
}

impl HeldRelay {
    /// A relay over `outbox`.
    pub fn new(outbox: adapter_memory::MemoryDirectoryOutbox) -> Self {
        let receiver = std::sync::Arc::new(Holding::default());
        let relay = DirectoryRelay::new(
            std::sync::Arc::new(outbox),
            std::sync::Arc::clone(&receiver),
            std::sync::Arc::new(Frozen),
            std::time::Duration::from_secs(2),
        );
        Self { receiver, relay }
    }
}

impl Delivery for HeldRelay {
    fn deliver(&self, repository: RepositoryId) -> impl Future<Output = ()> + Send {
        self.relay.deliver(repository)
    }
}

/// The draft name `name`.
pub fn draft(name: &str) -> DraftName {
    name.parse().unwrap()
}

impl<H: Deployment> Repo<'_, H> {
    /// Stands in for the client uploading `content`: puts the bytes where an upload grant leads.
    pub async fn upload(&self, content: &str) -> Address {
        self.harness
            .objects()
            .put(self.id, content.as_bytes().to_vec())
            .await
            .unwrap()
    }

    /// Uploads and stages each `(path, content)` as a regular file in `caller`'s draft `draft`.
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
    pub async fn main(&self) -> Option<Address> {
        self.harness
            .log()
            .read_ref(self.id, Ref::Main)
            .await
            .unwrap()
    }

    /// The content of the file at `path` in `main`'s tree, if one is there.
    pub async fn read(&self, path: &str) -> Option<String> {
        let path: Path = path.parse().unwrap();
        let derived = self.harness.derived();
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
                    .objects()
                    .get(self.id, file.content)
                    .await
                    .unwrap();
                Some(String::from_utf8(bytes).unwrap())
            }
            Some(Entry::Directory(_)) | None => None,
        }
    }
}
