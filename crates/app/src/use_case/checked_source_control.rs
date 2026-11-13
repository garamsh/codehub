//! Source control that queues a landed changeset's checks (ADR 0035).

use std::collections::BTreeMap;

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::draft::Edit;
use crate::domain::name::DraftName;
use crate::domain::path::{Path, Prefix};
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::port::driven::changeset_log::ChangesetLog;
use crate::port::driven::check_definitions::CheckDefinitions;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::job_queue::JobQueue;
use crate::port::driven::object_store::{Grant, ObjectStore};
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::source_control::{
    FileContent, LandError, Landing, ListedFile, PathDiff, SourceControl, SourceControlError,
    Version, Workspace,
};
use crate::use_case::landing_attempts::LandingAttempts;
use crate::use_case::post_land_checks::PostLandChecks;

/// The driven ports the landing of checked source control uses beyond those of the source
/// control it wraps.
pub struct CheckedAdapters<'a, O, D, L, R, Q, C> {
    /// Stores blobs.
    pub objects: &'a O,
    /// Stores derived data.
    pub derived: &'a D,
    /// Stores changesets and `last-green`.
    pub log: &'a L,
    /// Finds a repository's organisation.
    pub directory: &'a R,
    /// Holds the jobs.
    pub queue: &'a Q,
    /// Reads the check definitions.
    pub definitions: &'a C,
}

/// Source control that does what `S` does, and after each landing queues the checks the landed
/// changeset's tree defines. Where the landed changeset has no blocking check, `last-green`
/// moves to it at once.
pub struct CheckedSourceControl<'a, S, O, D, L, R, Q, C> {
    inner: S,
    checks: PostLandChecks<'a, O, D, L, R, Q, C>,
}

impl<'a, S, O, D, L, R, Q, C> CheckedSourceControl<'a, S, O, D, L, R, Q, C> {
    /// `inner`, with the checks of each of its landings queued through `adapters`. Where a
    /// landed changeset's check definitions cannot be read, it is reverted, each revert tried at
    /// most `attempts` times.
    pub const fn new(
        inner: S,
        adapters: CheckedAdapters<'a, O, D, L, R, Q, C>,
        attempts: LandingAttempts,
    ) -> Self {
        Self {
            inner,
            checks: PostLandChecks::new(adapters, attempts),
        }
    }
}

impl<S, O, D, L, R, Q, C> SourceControl for CheckedSourceControl<'_, S, O, D, L, R, Q, C>
where
    S: SourceControl + Sync,
    O: ObjectStore + Sync,
    D: DerivedData + Sync,
    L: ChangesetLog + Sync,
    R: RepositoryDirectory + Sync,
    Q: JobQueue + Sync,
    C: CheckDefinitions + Sync,
{
    async fn request_upload(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        address: Address,
        size: u64,
    ) -> Result<Grant, SourceControlError> {
        self.inner
            .request_upload(caller, repository, address, size)
            .await
    }

    async fn stage(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        path: Path,
        change: Change,
    ) -> Result<(), SourceControlError> {
        self.inner
            .stage(caller, repository, draft, path, change)
            .await
    }

    async fn open_workspace(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
    ) -> Result<Workspace, SourceControlError> {
        self.inner.open_workspace(caller, repository, draft).await
    }

    async fn workspace(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
    ) -> Result<Workspace, SourceControlError> {
        self.inner.workspace(caller, repository, draft).await
    }

    async fn describe(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        description: String,
    ) -> Result<(), SourceControlError> {
        self.inner
            .describe(caller, repository, draft, description)
            .await
    }

    async fn snapshot(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        edits: BTreeMap<Path, Edit>,
    ) -> Result<(), SourceControlError> {
        self.inner.snapshot(caller, repository, draft, edits).await
    }

    async fn restore(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        path: &Path,
    ) -> Result<(), SourceControlError> {
        self.inner.restore(caller, repository, draft, path).await
    }

    async fn diff(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        path: Option<&Path>,
    ) -> Result<Vec<PathDiff>, SourceControlError> {
        self.inner.diff(caller, repository, draft, path).await
    }

    async fn list_files(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        changeset: Address,
        prefix: &Prefix,
    ) -> Result<Vec<ListedFile>, SourceControlError> {
        self.inner
            .list_files(caller, repository, changeset, prefix)
            .await
    }

    async fn land(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        landing: Landing,
    ) -> Result<Address, SourceControlError> {
        let timestamp = landing.timestamp;
        let landed = self.inner.land(caller, repository, draft, landing).await?;
        self.checks
            .after_landing(repository, landed, timestamp)
            .await
            .map_err(|error| LandError::ChecksNotQueued {
                landed,
                cause: error.to_string(),
            })?;
        Ok(landed)
    }

    async fn read_file(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        path: &Path,
    ) -> Result<FileContent, SourceControlError> {
        self.inner.read_file(caller, repository, path).await
    }

    async fn history(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        at: Option<Address>,
        path: &Path,
    ) -> Result<Vec<Version>, SourceControlError> {
        self.inner.history(caller, repository, at, path).await
    }
}
