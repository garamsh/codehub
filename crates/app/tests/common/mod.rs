//! A workspace held by the in-memory adapters, with drafts written and landed through it.

use std::collections::BTreeMap;

use adapter_memory::{MemoryChangesetLog, MemoryDerivedData, MemoryDraftStore, MemoryObjectStore};
use app::domain::address::Address;
use app::domain::changeset::{Change, Changeset};
use app::domain::file::{File, FileKind};
use app::port::driven::changeset_log::{ChangesetLog, Ref};
use app::port::driven::draft_store::DraftStore;
use app::port::driven::object_store::ObjectStore;
use app::use_case::land::{LandError, land};

/// The in-memory adapters of one workspace.
#[derive(Default)]
pub struct Workspace {
    pub objects: MemoryObjectStore,
    pub log: MemoryChangesetLog,
    pub derived: MemoryDerivedData,
    pub drafts: MemoryDraftStore,
}

impl Workspace {
    /// Stores a draft based on `base` that writes each `(path, content)` as a regular file.
    pub async fn draft(
        &self,
        base: Option<Address>,
        message: &str,
        files: &[(&str, &str)],
    ) -> Address {
        let mut changes = BTreeMap::new();
        for (path, content) in files {
            let content = self.objects.put(content.as_bytes().to_vec()).await.unwrap();
            let file = File {
                kind: FileKind::Regular,
                content,
            };
            changes.insert(path.parse().unwrap(), Change::File(file));
        }
        let draft = Changeset {
            parents: base.into_iter().collect(),
            author: "ada".to_owned(),
            timestamp: 0,
            message: message.to_owned(),
            changes,
        };
        self.drafts.put(draft).await.unwrap()
    }

    /// Lands `draft` through the in-memory adapters.
    pub async fn land(&self, draft: Address) -> Result<Address, LandError> {
        land(&self.objects, &self.log, &self.derived, &self.drafts, draft).await
    }

    /// Where `main` points.
    pub async fn main(&self) -> Option<Address> {
        self.log.read_ref(Ref::Main).await.unwrap()
    }
}
