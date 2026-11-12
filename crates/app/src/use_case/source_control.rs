//! The source-control driving port, served by the upload, staging, landing, reading and history
//! use cases over the driven ports it is given. Each operation first checks, through the
//! authorisation decision, that the caller may do what it asks.

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::draft::DraftId;
use crate::domain::file_version::FileVersionNode;
use crate::domain::name::{DraftName, OrganisationName, RepositoryName};
use crate::domain::path::Path;
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::port::driven::authorisation_decision::{Action, AuthorisationDecision, Resource};
use crate::port::driven::changeset_log::ChangesetLog;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::draft_store::DraftStore;
use crate::port::driven::membership::Access;
use crate::port::driven::object_store::{Grant, ObjectStore};
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::access::AccessError;
use crate::port::driving::source_control::{
    FileContent, LandError, Landing, SourceControl, SourceControlError,
};
use crate::use_case::access::{readable_repository, require, require_paths};
use crate::use_case::history::history;
use crate::use_case::land::land;
use crate::use_case::read_file::read_file;
use crate::use_case::stage::stage;
use crate::use_case::upload::request_upload;

/// The driven ports source control is built over.
pub struct SourceControlAdapters<'a, O, L, D, S, R, A> {
    /// Stores blobs.
    pub objects: &'a O,
    /// Stores changesets and `main`.
    pub log: &'a L,
    /// Stores derived data.
    pub derived: &'a D,
    /// Stores drafts.
    pub drafts: &'a S,
    /// Finds a repository's organisation.
    pub directory: &'a R,
    /// Says what callers may do.
    pub authorisation: &'a A,
}

/// Source control over the driven ports the composition site passes to it.
pub struct SourceControlService<'a, O, L, D, S, R, A> {
    objects: &'a O,
    log: &'a L,
    derived: &'a D,
    drafts: &'a S,
    directory: &'a R,
    authorisation: &'a A,
    max_upload_size: u64,
}

impl<'a, O, L, D, S, R, A> SourceControlService<'a, O, L, D, S, R, A> {
    /// Source control over `adapters` that grants uploads of at most `max_upload_size` bytes.
    pub const fn new(
        adapters: SourceControlAdapters<'a, O, L, D, S, R, A>,
        max_upload_size: u64,
    ) -> Self {
        let SourceControlAdapters {
            objects,
            log,
            derived,
            drafts,
            directory,
            authorisation,
        } = adapters;
        Self {
            objects,
            log,
            derived,
            drafts,
            directory,
            authorisation,
            max_upload_size,
        }
    }
}

impl<O, L, D, S, R, A> SourceControlService<'_, O, L, D, S, R, A>
where
    R: RepositoryDirectory + Sync,
    A: AuthorisationDecision + Sync,
{
    /// The organisation of `repository` and its name there, where `caller` may read it.
    async fn names(
        &self,
        caller: &Caller,
        repository: RepositoryId,
    ) -> Result<(OrganisationName, RepositoryName), AccessError> {
        readable_repository(
            self.directory,
            self.authorisation,
            caller.principal(),
            repository,
        )
        .await
    }

    /// Requires `access` for `caller` on `path` in `repository`.
    async fn require_path(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        access: Access,
        path: &Path,
    ) -> Result<(), AccessError> {
        let (organisation, name) = self.names(caller, repository).await?;
        require_paths(
            self.authorisation,
            caller.principal(),
            &organisation,
            &name,
            repository,
            access,
            [path],
        )
        .await
    }
}

impl<O, L, D, S, R, A> SourceControl for SourceControlService<'_, O, L, D, S, R, A>
where
    O: ObjectStore + Sync,
    L: ChangesetLog + Sync,
    D: DerivedData + Sync,
    S: DraftStore + Sync,
    R: RepositoryDirectory + Sync,
    A: AuthorisationDecision + Sync,
{
    async fn request_upload(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        address: Address,
        size: u64,
    ) -> Result<Grant, SourceControlError> {
        let (organisation, name) = self.names(caller, repository).await?;
        require(
            self.authorisation,
            caller.principal(),
            Action::Write,
            &Resource::Repository {
                organisation,
                name,
                repository,
            },
        )
        .await?;
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
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        path: Path,
        change: Change,
    ) -> Result<(), SourceControlError> {
        self.require_path(caller, repository, Access::Write, &path)
            .await?;
        let draft = DraftId {
            principal: caller.principal().clone(),
            name: draft.clone(),
        };
        Ok(stage(
            self.objects,
            self.log,
            self.drafts,
            repository,
            &draft,
            path,
            change,
        )
        .await?)
    }

    async fn land(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        landing: Landing,
    ) -> Result<Address, SourceControlError> {
        let (organisation, name) = self.names(caller, repository).await?;
        let draft = DraftId {
            principal: caller.principal().clone(),
            name: draft.clone(),
        };
        let staged = self
            .drafts
            .get(repository, &draft)
            .await
            .map_err(LandError::from)?;
        require_paths(
            self.authorisation,
            caller.principal(),
            &organisation,
            &name,
            repository,
            Access::Write,
            staged.changes.keys(),
        )
        .await
        .map_err(LandError::from)?;
        let landed = land(
            self.objects,
            self.log,
            self.derived,
            repository,
            caller.principal(),
            staged,
            landing,
        )
        .await?;
        self.drafts
            .remove(repository, &draft)
            .await
            .map_err(LandError::from)?;
        Ok(landed)
    }

    async fn read_file(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        path: &Path,
    ) -> Result<FileContent, SourceControlError> {
        self.require_path(caller, repository, Access::Read, path)
            .await?;
        Ok(read_file(self.objects, self.log, self.derived, repository, path).await?)
    }

    async fn history(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        at: Option<Address>,
        path: &Path,
    ) -> Result<Vec<FileVersionNode>, SourceControlError> {
        self.require_path(caller, repository, Access::Read, path)
            .await?;
        Ok(history(self.log, self.derived, repository, at, path).await?)
    }
}
