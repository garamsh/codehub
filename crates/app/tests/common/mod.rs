//! Repositories held by the in-memory adapters, with drafts staged and landed through the driving
//! ports.

use adapter_memory::{
    MemoryChangesetLog, MemoryDerivedData, MemoryDraftStore, MemoryObjectStore,
    MemoryRepositoryDirectory,
};
use app::domain::address::Address;
use app::domain::changeset::Change;
use app::domain::draft::DraftId;
use app::domain::file::{File, FileKind};
use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::path::Path;
use app::domain::repository::RepositoryId;
use app::domain::tree::Entry;
use app::port::driven::changeset_log::{ChangesetLog, Ref};
use app::port::driven::derived_data::DerivedData;
use app::port::driven::object_store::ObjectStore;
use app::port::driving::repositories::Repositories;
use app::port::driving::source_control::{SourceControl, SourceControlError};
use app::use_case::land::Landing;
use app::use_case::repositories::RepositoriesService;
use app::use_case::source_control::SourceControlService;

/// The largest upload the fixture's source control grants.
pub const MAX_UPLOAD_SIZE: u64 = 1024;

/// The in-memory adapters of one deployment, holding any number of repositories.
#[derive(Default)]
pub struct Harness {
    pub objects: MemoryObjectStore,
    pub log: MemoryChangesetLog,
    pub derived: MemoryDerivedData,
    pub drafts: MemoryDraftStore,
    pub directory: MemoryRepositoryDirectory,
}

/// The source-control service over the harness's adapters.
pub type Service<'a> = SourceControlService<
    'a,
    MemoryObjectStore,
    MemoryChangesetLog,
    MemoryDerivedData,
    MemoryDraftStore,
>;

impl Harness {
    /// The repositories port over this harness's directory.
    pub const fn repositories(&self) -> RepositoriesService<'_, MemoryRepositoryDirectory> {
        RepositoriesService::new(&self.directory)
    }

    /// Source control over this harness's adapters.
    pub const fn source_control(&self) -> Service<'_> {
        SourceControlService::new(
            &self.objects,
            &self.log,
            &self.derived,
            &self.drafts,
            MAX_UPLOAD_SIZE,
        )
    }

    /// Creates the organisation `organisation` holding the repository `app`, through the
    /// repositories port.
    pub async fn repository(&self, organisation: &str) -> Repo<'_> {
        let organisation: OrganisationName = organisation.parse().unwrap();
        let name: RepositoryName = "app".parse().unwrap();
        self.repositories()
            .create_organisation(&organisation)
            .await
            .unwrap();
        let id = self
            .repositories()
            .create_repository(&organisation, &name)
            .await
            .unwrap();
        Repo { harness: self, id }
    }
}

/// One repository of a harness.
pub struct Repo<'a> {
    pub harness: &'a Harness,
    pub id: RepositoryId,
}

/// The draft `name` of the principal `principal`.
pub fn draft_id(principal: &str, name: &str) -> DraftId {
    DraftId {
        principal: principal.parse().unwrap(),
        name: name.parse().unwrap(),
    }
}

impl Repo<'_> {
    /// Stands in for the client uploading `content`: puts the bytes where an upload grant leads.
    pub async fn upload(&self, content: &str) -> Address {
        self.harness
            .objects
            .put(self.id, content.as_bytes().to_vec())
            .await
            .unwrap()
    }

    /// Uploads and stages each `(path, content)` as a regular file in the draft `draft`.
    pub async fn stage(&self, draft: &DraftId, files: &[(&str, &str)]) {
        for (path, content) in files {
            let content = self.upload(content).await;
            let file = File {
                kind: FileKind::Regular,
                content,
            };
            self.harness
                .source_control()
                .stage(self.id, draft, path.parse().unwrap(), Change::File(file))
                .await
                .unwrap();
        }
    }

    /// Lands `draft` through the source-control port.
    pub async fn land(
        &self,
        draft: &DraftId,
        message: &str,
    ) -> Result<Address, SourceControlError> {
        let landing = Landing {
            message: message.to_owned(),
            timestamp: 0,
        };
        self.harness
            .source_control()
            .land(self.id, draft, landing)
            .await
    }

    /// Where `main` points.
    pub async fn main(&self) -> Option<Address> {
        self.harness.log.read_ref(self.id, Ref::Main).await.unwrap()
    }

    /// The content of the file at `path` in `main`'s tree, if one is there.
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
