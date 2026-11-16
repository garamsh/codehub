//! The jobs of runs: what a runner is sent when it takes one, and what the server does when the
//! runner reports it (ADR 0051).

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::draft::{Draft, DraftId, Edit};
use crate::domain::file::File;
use crate::domain::path::{Path, Prefix};
use crate::domain::repository::RepositoryId;
use crate::domain::run::{NotWritten, Run, RunOutcome, RunReport, RunResult, WriteBack};
use crate::domain::runner::{Registration, RegistrationId};
use crate::port::driven::authorisation_decision::AuthorisationDecision;
use crate::port::driven::cache_refs::CacheRefs;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::draft_store::{DraftStore, DraftStoreError};
use crate::port::driven::job_queue::JobQueue;
use crate::port::driven::membership::Access;
use crate::port::driven::object_store::ObjectStore;
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::access::AccessError;
use crate::port::driving::jobs::{ClaimedCache, ClaimedRun, JobsError};
use crate::use_case::access::require_paths;
use crate::use_case::cache_refs::{CacheOutcome, move_workspace_cache_refs};
use crate::use_case::jobs::{claimed_file, covered, relative, store_tree, walk_tree};
use crate::use_case::list_files::files_under;
use crate::use_case::runs::workspace_files;

/// Hands out the jobs of runs and ends them.
pub(crate) struct RunJobs {
    queue: Arc<dyn JobQueue>,
    cache_refs: Arc<dyn CacheRefs>,
    objects: Arc<dyn ObjectStore>,
    derived: Arc<dyn DerivedData>,
    drafts: Arc<dyn DraftStore>,
    authorisation: Arc<dyn AuthorisationDecision>,
    directory: Arc<dyn RepositoryDirectory>,
}

impl RunJobs {
    pub(crate) fn new(
        queue: Arc<dyn JobQueue>,
        cache_refs: Arc<dyn CacheRefs>,
        objects: Arc<dyn ObjectStore>,
        derived: Arc<dyn DerivedData>,
        drafts: Arc<dyn DraftStore>,
        authorisation: Arc<dyn AuthorisationDecision>,
        directory: Arc<dyn RepositoryDirectory>,
    ) -> Self {
        Self {
            queue,
            cache_refs,
            objects,
            derived,
            drafts,
            authorisation,
            directory,
        }
    }

    /// Takes the oldest queued run `registrations` may take, with the files it is sent.
    pub(crate) async fn claim(
        &self,
        registrations: &[Registration],
        now: i64,
        lease: i64,
    ) -> Result<Option<ClaimedRun>, JobsError> {
        let Some(run) = self.queue.claim_run(registrations, now, lease).await? else {
            return Ok(None);
        };
        let repository = run.request.repository;
        let mut files = Vec::new();
        for (path, file) in &run.request.files {
            files.push(claimed_file(&*self.objects, repository, path.clone(), *file).await?);
        }
        let mut caches = Vec::new();
        for (path, tree) in &run.request.caches {
            let mut cached = Vec::new();
            for (inside, file) in walk_tree::<JobsError>(&*self.objects, repository, *tree).await? {
                cached.push(claimed_file(&*self.objects, repository, inside, file).await?);
            }
            caches.push(ClaimedCache {
                path: path.clone(),
                files: cached,
            });
        }
        Ok(Some(ClaimedRun { run, files, caches }))
    }

    /// Ends `held`, which runs under one of `runners`, as `report` says.
    pub(crate) async fn report(
        &self,
        runners: &[RegistrationId],
        held: Run,
        report: RunReport,
    ) -> Result<(), JobsError> {
        let request = &held.request;
        let repository = request.repository;
        if !self.stored(repository, &report).await? {
            let result =
                RunResult::unreported(RunOutcome::ExecutorFailed, NotWritten::ExecutorFailed);
            self.queue.complete_run(held.id, runners, result).await?;
            return Ok(());
        }
        let outputs: Vec<Prefix> = request.outputs.iter().cloned().map(Prefix::from).collect();
        let caches: Vec<Prefix> = request.caches.keys().cloned().map(Prefix::from).collect();
        let mut left = covered(&report.outputs, &outputs);
        let cache_files = covered(&report.caches, &caches);
        left.extend(cache_files.clone());
        let output_tree = store_tree(&*self.objects, repository, left).await?;

        let draft = self.open_draft(&held).await?;
        let write_back = match report.outcome {
            RunOutcome::Exited => {
                self.write_back(&held, draft.as_ref(), &report.changes)
                    .await?
            }
            RunOutcome::TimedOut => not_written(NotWritten::TimedOut),
            RunOutcome::Cancelled => not_written(NotWritten::Cancelled),
            RunOutcome::ExecutorFailed => not_written(NotWritten::ExecutorFailed),
        };
        // A landing removes its instance's cache refs, so the swap finds none for a closed one.
        if report.outcome == RunOutcome::Exited {
            let mut moved = Vec::new();
            for (path, started) in &request.caches {
                let prefix = Prefix::from(path.clone());
                let files = cache_files
                    .iter()
                    .filter_map(|(file, content)| Some((relative(&prefix, file)?, *content)))
                    .collect();
                let tree = store_tree(&*self.objects, repository, files).await?;
                moved.push(CacheOutcome {
                    path: path.clone(),
                    started: *started,
                    left: tree,
                });
            }
            move_workspace_cache_refs(&*self.cache_refs, repository, request.instance, &moved)
                .await?;
        }
        let result = RunResult {
            outcome: report.outcome,
            exit_code: report.exit_code,
            stdout: report.stdout,
            stderr: report.stderr,
            objects: report.objects,
            output_tree: Some(output_tree),
            changes: report.changes,
            refused: report.refused,
            write_back,
        };
        self.queue.complete_run(held.id, runners, result).await?;
        Ok(())
    }

    /// Whether every object `report` names is in the object store.
    async fn stored(
        &self,
        repository: RepositoryId,
        report: &RunReport,
    ) -> Result<bool, JobsError> {
        let streams = report
            .stdout
            .iter()
            .chain(&report.stderr)
            .map(|stream| stream.address);
        let produced = report
            .outputs
            .iter()
            .chain(&report.caches)
            .map(|file| file.file.content);
        let changed = report.changes.values().filter_map(|change| match change {
            Change::File(file) => Some(file.content),
            Change::Deletion => None,
        });
        for address in streams.chain(produced).chain(changed) {
            if !self.objects.exists(repository, address).await? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The workspace of `held`'s principal, where it is still the instance `held` started in.
    async fn open_draft(&self, held: &Run) -> Result<Option<Draft>, JobsError> {
        let id = DraftId {
            principal: held.request.principal.clone(),
            name: held.request.draft.clone(),
        };
        match self.drafts.get(held.request.repository, &id).await {
            Ok(draft) if draft.instance == held.request.instance => Ok(Some(draft)),
            Ok(_) | Err(DraftStoreError::NotFound(_)) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Writes `changes` to the workspace `draft` as one snapshot, where ADR 0051 lets it.
    async fn write_back(
        &self,
        held: &Run,
        draft: Option<&Draft>,
        changes: &BTreeMap<Path, Change>,
    ) -> Result<WriteBack, JobsError> {
        if changes.is_empty() {
            return Ok(WriteBack::Written { paths: Vec::new() });
        }
        let request = &held.request;
        let Some(draft) = draft else {
            return Ok(refused(NotWritten::InstanceClosed, Vec::new()));
        };
        if draft.version != request.version {
            let paths = self.moved(held, draft).await?;
            return Ok(refused(NotWritten::WorkspaceMoved, paths));
        }
        let repository = request.repository;
        let Some((organisation, name)) = self
            .directory
            .names_of(repository)
            .await
            .map_err(AccessError::from)?
        else {
            return Err(AccessError::RepositoryNotFound(repository).into());
        };
        match require_paths(
            &*self.authorisation,
            &request.principal,
            &organisation,
            &name,
            repository,
            Access::Write,
            changes.keys(),
        )
        .await
        {
            Ok(()) => {}
            Err(AccessError::PathsDenied { paths, .. }) => {
                return Ok(refused(
                    NotWritten::WriteDenied,
                    paths.into_iter().collect(),
                ));
            }
            Err(error) => return Err(error.into()),
        }
        let mut edits = BTreeMap::new();
        for (path, change) in changes {
            let base = self.base_file(repository, draft.base, path).await?;
            let edit = match (change, base) {
                (Change::File(file), Some(base)) if *file == base => Edit::Restore,
                (Change::Deletion, None) => Edit::Restore,
                (change, _) => Edit::Record(*change),
            };
            edits.insert(path.clone(), edit);
        }
        let id = DraftId {
            principal: request.principal.clone(),
            name: request.draft.clone(),
        };
        match self
            .drafts
            .record(repository, &id, request.version, edits)
            .await
        {
            Ok(()) => Ok(WriteBack::Written {
                paths: changes.keys().cloned().collect(),
            }),
            Err(DraftStoreError::Moved { .. }) => {
                let paths = self.moved(held, draft).await?;
                Ok(refused(NotWritten::WorkspaceMoved, paths))
            }
            Err(DraftStoreError::NotFound(_)) => {
                Ok(refused(NotWritten::InstanceClosed, Vec::new()))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// The file at `path` in the changeset `base`, where it holds one.
    async fn base_file(
        &self,
        repository: RepositoryId,
        base: Option<Address>,
        path: &Path,
    ) -> Result<Option<File>, JobsError> {
        let Some(base) = base else {
            return Ok(None);
        };
        let root = self.derived.root(repository, base).await?;
        let found = files_under(
            &*self.derived,
            repository,
            root,
            &Prefix::from(path.clone()),
        )
        .await?;
        Ok(found
            .into_iter()
            .find(|(found, _)| found == path)
            .map(|(_, file)| file))
    }

    /// The paths under the run's prefix whose files the workspace holds otherwise than the run
    /// was sent them.
    async fn moved(&self, held: &Run, draft: &Draft) -> Result<Vec<Path>, JobsError> {
        let request = &held.request;
        let now =
            workspace_files(&*self.derived, request.repository, draft, &request.prefix).await?;
        let mut paths: Vec<Path> = now
            .iter()
            .filter(|(path, file)| request.files.get(*path) != Some(*file))
            .map(|(path, _)| path.clone())
            .collect();
        paths.extend(
            request
                .files
                .keys()
                .filter(|path| !now.contains_key(*path))
                .cloned(),
        );
        paths.sort();
        Ok(paths)
    }
}

fn not_written(reason: NotWritten) -> WriteBack {
    refused(reason, Vec::new())
}

fn refused(reason: NotWritten, paths: Vec<Path>) -> WriteBack {
    WriteBack::NotWritten { reason, paths }
}
