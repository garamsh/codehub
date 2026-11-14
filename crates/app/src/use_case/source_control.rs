//! The source-control driving port, served by the upload, staging, workspace, landing, reading and
//! history use cases over the driven ports it is given. Each operation first checks, through the
//! authorisation decision, that the caller may do what it asks.

use std::collections::BTreeMap;
use std::future::Future;

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::draft::{Draft, DraftId, Edit};
use crate::domain::name::{DraftName, OrganisationName, RepositoryName};
use crate::domain::path::{Path, Prefix};
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::port::driven::authorisation_decision::{Action, AuthorisationDecision, Resource};
use crate::port::driven::changeset_log::{ChangesetLog, Ref};
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::draft_store::DraftStore;
use crate::port::driven::membership::Access;
use crate::port::driven::object_store::{Grant, ObjectStore};
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::access::AccessError;
use crate::port::driving::source_control::{
    FileContent, LandError, Landing, ListFilesError, ListedFile, PathDiff, SourceControl,
    SourceControlError, Version, Workspace, WorkspaceError,
};
use crate::use_case::access::{readable_repository, require, require_paths};
use crate::use_case::history::history;
use crate::use_case::land::land;
use crate::use_case::landing_attempts::LandingAttempts;
use crate::use_case::landing_gates::LandingGates;
use crate::use_case::list_files::files_under;
use crate::use_case::post_land_checks::AfterLandingError;
use crate::use_case::read_file::read_file;
use crate::use_case::upload::request_upload;
use crate::use_case::workspace::{changed, diffs, require_uploaded, workspace};

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

/// What happens after a changeset has landed on `main`.
pub trait AfterLanding {
    /// Acts on `landed` having landed in `repository`, with `timestamp` as the time of anything
    /// it records.
    ///
    /// # Errors
    ///
    /// [`AfterLandingError`] where what follows the landing could not be done; the changeset has
    /// landed all the same.
    fn after_landing(
        &self,
        repository: RepositoryId,
        landed: Address,
        timestamp: i64,
    ) -> impl Future<Output = Result<(), AfterLandingError>> + Send;
}

/// The step after a landing that does nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoAfterLanding;

impl AfterLanding for NoAfterLanding {
    async fn after_landing(
        &self,
        _repository: RepositoryId,
        _landed: Address,
        _timestamp: i64,
    ) -> Result<(), AfterLandingError> {
        Ok(())
    }
}

/// Source control over the driven ports the composition site passes to it, running `P` after
/// each landing.
pub struct SourceControlService<'a, O, L, D, S, R, A, P = NoAfterLanding> {
    objects: &'a O,
    log: &'a L,
    derived: &'a D,
    drafts: &'a S,
    directory: &'a R,
    authorisation: &'a A,
    max_upload_size: u64,
    attempts: LandingAttempts,
    after_landing: P,
    /// One gate per repository, held while a landing of it rebases and swaps: the landings of one
    /// repository wait for one another within this service, and not across server processes.
    gates: LandingGates,
}

impl<'a, O, L, D, S, R, A> SourceControlService<'a, O, L, D, S, R, A> {
    /// Source control over `adapters` that grants uploads of at most `max_upload_size` bytes,
    /// tries each landing as often as [`LandingAttempts::default`] says and does nothing after
    /// a landing.
    pub fn new(
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
            attempts: LandingAttempts::new(LandingAttempts::DEFAULT),
            after_landing: NoAfterLanding,
            gates: LandingGates::new(),
        }
    }
}

impl<'a, O, L, D, S, R, A, P> SourceControlService<'a, O, L, D, S, R, A, P> {
    /// This source control with each landing that lost the compare-and-swap on `main` tried at
    /// most `attempts` times.
    #[must_use]
    pub fn with_landing_attempts(self, attempts: LandingAttempts) -> Self {
        Self { attempts, ..self }
    }

    /// This source control running `after_landing` after each landing.
    #[must_use]
    pub fn with_after_landing<N>(
        self,
        after_landing: N,
    ) -> SourceControlService<'a, O, L, D, S, R, A, N> {
        let Self {
            objects,
            log,
            derived,
            drafts,
            directory,
            authorisation,
            max_upload_size,
            attempts,
            gates,
            after_landing: _,
        } = self;
        SourceControlService {
            objects,
            log,
            derived,
            drafts,
            directory,
            authorisation,
            max_upload_size,
            attempts,
            after_landing,
            gates,
        }
    }
}

impl<O, L, D, S, R, A, P> SourceControlService<'_, O, L, D, S, R, A, P>
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

impl<O, L, D, S, R, A, P> SourceControlService<'_, O, L, D, S, R, A, P>
where
    O: ObjectStore + Sync,
    D: DerivedData + Sync,
    R: RepositoryDirectory + Sync,
    A: AuthorisationDecision + Sync,
{
    /// `draft` as a workspace, where `caller` may read every path it changes.
    async fn read_workspace(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        organisation: &OrganisationName,
        name: &RepositoryName,
        draft: &Draft,
    ) -> Result<Workspace, SourceControlError> {
        let changed = changed(self.derived, repository, draft)
            .await
            .map_err(WorkspaceError::from)?;
        require_paths(
            self.authorisation,
            caller.principal(),
            organisation,
            name,
            repository,
            Access::Read,
            changed.iter().map(|one| one.path),
        )
        .await?;
        Ok(workspace(self.objects, repository, draft, &changed).await?)
    }
}

fn draft_id(caller: &Caller, draft: &DraftName) -> DraftId {
    DraftId {
        principal: caller.principal().clone(),
        name: draft.clone(),
    }
}

impl<O, L, D, S, R, A, P> SourceControl for SourceControlService<'_, O, L, D, S, R, A, P>
where
    O: ObjectStore + Sync,
    L: ChangesetLog + Sync,
    D: DerivedData + Sync,
    S: DraftStore + Sync,
    R: RepositoryDirectory + Sync,
    A: AuthorisationDecision + Sync,
    P: AfterLanding + Sync,
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
        let edits = BTreeMap::from([(path, Edit::Record(change))]);
        require_uploaded(self.objects, repository, &edits).await?;
        let draft = draft_id(caller, draft);
        let base = self
            .log
            .read_ref(repository, Ref::Main)
            .await
            .map_err(WorkspaceError::from)?;
        self.drafts
            .open(repository, &draft, base)
            .await
            .map_err(WorkspaceError::from)?;
        self.drafts
            .record(repository, &draft, edits)
            .await
            .map_err(WorkspaceError::from)?;
        Ok(())
    }

    async fn open_workspace(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
    ) -> Result<Workspace, SourceControlError> {
        let (organisation, name) = self.names(caller, repository).await?;
        require(
            self.authorisation,
            caller.principal(),
            Action::Write,
            &Resource::Repository {
                organisation: organisation.clone(),
                name: name.clone(),
                repository,
            },
        )
        .await?;
        let base = self
            .log
            .read_ref(repository, Ref::Main)
            .await
            .map_err(WorkspaceError::from)?;
        let opened = self
            .drafts
            .open(repository, &draft_id(caller, draft), base)
            .await
            .map_err(WorkspaceError::from)?;
        self.read_workspace(caller, repository, &organisation, &name, &opened)
            .await
    }

    async fn workspace(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
    ) -> Result<Workspace, SourceControlError> {
        let (organisation, name) = self.names(caller, repository).await?;
        let held = self
            .drafts
            .get(repository, &draft_id(caller, draft))
            .await
            .map_err(WorkspaceError::from)?;
        self.read_workspace(caller, repository, &organisation, &name, &held)
            .await
    }

    async fn describe(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        description: String,
    ) -> Result<(), SourceControlError> {
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
        self.drafts
            .describe(repository, &draft_id(caller, draft), description)
            .await
            .map_err(WorkspaceError::from)?;
        Ok(())
    }

    async fn snapshot(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        edits: BTreeMap<Path, Edit>,
    ) -> Result<(), SourceControlError> {
        let (organisation, name) = self.names(caller, repository).await?;
        require_paths(
            self.authorisation,
            caller.principal(),
            &organisation,
            &name,
            repository,
            Access::Write,
            edits.keys(),
        )
        .await?;
        require_uploaded(self.objects, repository, &edits).await?;
        self.drafts
            .record(repository, &draft_id(caller, draft), edits)
            .await
            .map_err(WorkspaceError::from)?;
        Ok(())
    }

    async fn restore(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        path: &Path,
    ) -> Result<(), SourceControlError> {
        self.require_path(caller, repository, Access::Write, path)
            .await?;
        self.drafts
            .record(
                repository,
                &draft_id(caller, draft),
                BTreeMap::from([(path.clone(), Edit::Restore)]),
            )
            .await
            .map_err(WorkspaceError::from)?;
        Ok(())
    }

    async fn diff(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        path: Option<&Path>,
    ) -> Result<Vec<PathDiff>, SourceControlError> {
        let (organisation, name) = self.names(caller, repository).await?;
        let held = self
            .drafts
            .get(repository, &draft_id(caller, draft))
            .await
            .map_err(WorkspaceError::from)?;
        let mut changed = changed(self.derived, repository, &held)
            .await
            .map_err(WorkspaceError::from)?;
        changed.retain(|one| path.is_none_or(|wanted| one.path == wanted));
        require_paths(
            self.authorisation,
            caller.principal(),
            &organisation,
            &name,
            repository,
            Access::Read,
            changed.iter().map(|one| one.path),
        )
        .await?;
        Ok(diffs(self.objects, repository, &changed).await?)
    }

    async fn list_files(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        changeset: Address,
        prefix: &Prefix,
    ) -> Result<Vec<ListedFile>, SourceControlError> {
        let (organisation, name) = self.names(caller, repository).await?;
        let root = self
            .derived
            .root(repository, changeset)
            .await
            .map_err(ListFilesError::from)?;
        let files = files_under(self.derived, repository, root, prefix)
            .await
            .map_err(ListFilesError::from)?;
        require_paths(
            self.authorisation,
            caller.principal(),
            &organisation,
            &name,
            repository,
            Access::Read,
            files.iter().map(|(path, _)| path),
        )
        .await?;
        let mut listed = Vec::new();
        for (path, file) in files {
            listed.push(ListedFile {
                path,
                kind: file.kind,
                content: file.content,
                download: self
                    .objects
                    .download_grant(repository, file.content)
                    .await
                    .map_err(ListFilesError::from)?,
            });
        }
        Ok(listed)
    }

    async fn land(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        landing: Landing,
    ) -> Result<Address, SourceControlError> {
        let (organisation, name) = self.names(caller, repository).await?;
        let draft = draft_id(caller, draft);
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
        let landing = if landing.message.is_empty() {
            Landing {
                message: staged.description.clone(),
                ..landing
            }
        } else {
            landing
        };
        let timestamp = landing.timestamp;
        let arrival = self.gates.reach(repository);
        let _one_at_a_time = arrival.pass().await;
        let landed = land(
            self.objects,
            self.log,
            self.derived,
            repository,
            caller.principal(),
            staged,
            landing,
            self.attempts,
        )
        .await?;
        self.drafts
            .remove(repository, &draft)
            .await
            .map_err(LandError::from)?;
        self.after_landing
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
    ) -> Result<Vec<Version>, SourceControlError> {
        self.require_path(caller, repository, Access::Read, path)
            .await?;
        Ok(history(self.log, self.derived, repository, at, path).await?)
    }
}
