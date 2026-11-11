//! The source-control driving port, served by the upload, staging, landing, reading and history
//! use cases over the driven ports it is given.

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::draft::DraftId;
use crate::domain::file_version::FileVersionNode;
use crate::domain::path::Path;
use crate::domain::repository::RepositoryId;
use crate::port::driven::changeset_log::ChangesetLog;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::draft_store::DraftStore;
use crate::port::driven::object_store::{Grant, ObjectStore};
use crate::port::driving::source_control::{SourceControl, SourceControlError};
use crate::use_case::history::history;
use crate::use_case::land::{Landing, land};
use crate::use_case::read_file::{FileContent, read_file};
use crate::use_case::stage::stage;
use crate::use_case::upload::request_upload;

/// Source control over the driven ports the composition site passes to it.
pub struct SourceControlService<'a, O, L, D, S> {
    objects: &'a O,
    log: &'a L,
    derived: &'a D,
    drafts: &'a S,
    max_upload_size: u64,
}

impl<'a, O, L, D, S> SourceControlService<'a, O, L, D, S> {
    /// Source control that stores blobs in `objects`, changesets and `main` in `log`, derived
    /// data in `derived`, and drafts in `drafts`, and grants uploads of at most `max_upload_size`
    /// bytes.
    pub const fn new(
        objects: &'a O,
        log: &'a L,
        derived: &'a D,
        drafts: &'a S,
        max_upload_size: u64,
    ) -> Self {
        Self {
            objects,
            log,
            derived,
            drafts,
            max_upload_size,
        }
    }
}

impl<O, L, D, S> SourceControl for SourceControlService<'_, O, L, D, S>
where
    O: ObjectStore + Sync,
    L: ChangesetLog + Sync,
    D: DerivedData + Sync,
    S: DraftStore + Sync,
{
    async fn request_upload(
        &self,
        repository: RepositoryId,
        address: Address,
        size: u64,
    ) -> Result<Grant, SourceControlError> {
        Ok(request_upload(
            self.objects,
            repository,
            address,
            size,
            self.max_upload_size,
        )
        .await?)
    }

    async fn stage(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        path: Path,
        change: Change,
    ) -> Result<(), SourceControlError> {
        Ok(stage(
            self.objects,
            self.log,
            self.drafts,
            repository,
            draft,
            path,
            change,
        )
        .await?)
    }

    async fn land(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        landing: Landing,
    ) -> Result<Address, SourceControlError> {
        Ok(land(
            self.objects,
            self.log,
            self.derived,
            self.drafts,
            repository,
            draft,
            landing,
        )
        .await?)
    }

    async fn read_file(
        &self,
        repository: RepositoryId,
        path: &Path,
    ) -> Result<FileContent, SourceControlError> {
        Ok(read_file(self.objects, self.log, self.derived, repository, path).await?)
    }

    async fn history(
        &self,
        repository: RepositoryId,
        at: Address,
        path: &Path,
    ) -> Result<Vec<FileVersionNode>, SourceControlError> {
        Ok(history(self.log, self.derived, repository, at, path).await?)
    }
}
