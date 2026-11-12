//! Source control that queues a landed changeset's checks (ADR 0035).

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::check::CheckDefinition;
use crate::domain::file_version::FileVersionNode;
use crate::domain::job::NewJob;
use crate::domain::name::DraftName;
use crate::domain::path::Path;
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::port::driven::changeset_log::ChangesetLog;
use crate::port::driven::check_definitions::CheckDefinitions;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::job_queue::JobQueue;
use crate::port::driven::object_store::{Grant, ObjectStore};
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::source_control::{
    FileContent, LandError, Landing, SourceControl, SourceControlError,
};
use crate::use_case::last_green::advance;
use crate::use_case::read_file::file_at;
use crate::use_case::revert::revert_on_blocking_failure;

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
    adapters: CheckedAdapters<'a, O, D, L, R, Q, C>,
}

impl<'a, S, O, D, L, R, Q, C> CheckedSourceControl<'a, S, O, D, L, R, Q, C> {
    /// `inner`, with the checks of each of its landings queued through `adapters`.
    pub const fn new(inner: S, adapters: CheckedAdapters<'a, O, D, L, R, Q, C>) -> Self {
        Self { inner, adapters }
    }
}

impl<S, O, D, L, R, Q, C> CheckedSourceControl<'_, S, O, D, L, R, Q, C>
where
    O: ObjectStore + Sync,
    D: DerivedData + Sync,
    L: ChangesetLog + Sync,
    R: RepositoryDirectory + Sync,
    Q: JobQueue + Sync,
    C: CheckDefinitions + Sync,
{
    /// Queues the checks `landed` defines, or moves `last-green` to it where none blocks. The
    /// error is a description of what failed.
    async fn queue_checks(&self, repository: RepositoryId, landed: Address) -> Result<(), String> {
        let adapters = &self.adapters;
        let (organisation, _) = adapters
            .directory
            .names_of(repository)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("no repository {repository}"))?;
        let job = |check: CheckDefinition| NewJob {
            organisation: organisation.clone(),
            repository,
            changeset: landed,
            check,
        };

        let file = adapters.definitions.file();
        let root = adapters
            .derived
            .root(repository, landed)
            .await
            .map_err(|error| error.to_string())?;
        let found = file_at(adapters.derived, repository, Some(root), file)
            .await
            .map_err(|error| error.to_string())?;
        let definitions = match found {
            None => Vec::new(),
            Some(found) => {
                let bytes = adapters
                    .objects
                    .get(repository, found.content)
                    .await
                    .map_err(|error| error.to_string())?;
                match adapters.definitions.parse(&bytes) {
                    Ok(definitions) => definitions,
                    Err(error) => {
                        let unreadable = job(CheckDefinition::unreadable(file));
                        let failed = adapters
                            .queue
                            .record_failed(unreadable, error.to_string().into_bytes())
                            .await
                            .map_err(|error| error.to_string())?;
                        revert_on_blocking_failure(repository, &failed);
                        return Ok(());
                    }
                }
            }
        };

        let blocking = definitions.iter().any(|check| check.blocking);
        adapters
            .queue
            .queue(definitions.into_iter().map(job).collect())
            .await
            .map_err(|error| error.to_string())?;
        if !blocking {
            advance(adapters.log, repository, landed)
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
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

    async fn land(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        landing: Landing,
    ) -> Result<Address, SourceControlError> {
        let landed = self.inner.land(caller, repository, draft, landing).await?;
        self.queue_checks(repository, landed)
            .await
            .map_err(|cause| LandError::ChecksNotQueued { landed, cause })?;
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
    ) -> Result<Vec<FileVersionNode>, SourceControlError> {
        self.inner.history(caller, repository, at, path).await
    }
}
