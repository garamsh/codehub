//! Runs: starting one in a caller's workspace, and answering it to the principal who started it
//! (ADR 0051).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::domain::changeset::Change;
use crate::domain::draft::{Draft, DraftId};
use crate::domain::file::File;
use crate::domain::job::empty_output_tree;
use crate::domain::name::DraftName;
use crate::domain::path::{Path, Prefix};
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::domain::run::{NewRun, Run, RunId, RunSettings, RunStream};
use crate::port::driven::authorisation_decision::{Action, AuthorisationDecision, Resource};
use crate::port::driven::cache_refs::CacheRefs;
use crate::port::driven::changeset_log::{ChangesetLog, Ref};
use crate::port::driven::check_definitions::CheckDefinitions;
use crate::port::driven::derived_data::{DerivedData, DerivedDataError};
use crate::port::driven::draft_store::DraftStore;
use crate::port::driven::job_queue::{JobQueue, JobQueueError};
use crate::port::driven::membership::Access;
use crate::port::driven::object_store::ObjectStore;
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::runs::{OutputFile, Runs, RunsError, StartRun, StreamDownloads};
use crate::use_case::access::{readable_repository, require, require_paths};
use crate::use_case::cache_refs::create_workspace_cache_ref;
use crate::use_case::jobs::walk_tree;
use crate::use_case::list_files::files_under;
use crate::use_case::tree_checks::tree_files;

/// The driven ports the runs are built over.
pub struct RunsAdapters {
    /// Holds the runs.
    pub queue: Arc<dyn JobQueue>,
    /// Finds a repository's organisation and name.
    pub directory: Arc<dyn RepositoryDirectory>,
    /// Says what callers may do.
    pub authorisation: Arc<dyn AuthorisationDecision>,
    /// Stores drafts.
    pub drafts: Arc<dyn DraftStore>,
    /// Stores derived data.
    pub derived: Arc<dyn DerivedData>,
    /// Stores blobs.
    pub objects: Arc<dyn ObjectStore>,
    /// Stores `main`.
    pub log: Arc<dyn ChangesetLog>,
    /// Holds the cache refs.
    pub cache_refs: Arc<dyn CacheRefs>,
    /// Reads the run settings.
    pub definitions: Arc<dyn CheckDefinitions>,
}

/// The files of `draft` under `prefix`: its base's, with its changes applied.
pub(crate) async fn workspace_files(
    derived: &dyn DerivedData,
    repository: RepositoryId,
    draft: &Draft,
    prefix: &Prefix,
) -> Result<BTreeMap<Path, File>, DerivedDataError> {
    let mut files = BTreeMap::new();
    if let Some(base) = draft.base {
        let root = derived.root(repository, base).await?;
        files.extend(files_under(derived, repository, root, prefix).await?);
    }
    for (path, change) in &draft.changes {
        if !prefix.covers(path) {
            continue;
        }
        match change {
            Change::File(file) => files.insert(path.clone(), *file),
            Change::Deletion => files.remove(path),
        };
    }
    Ok(files)
}

/// The runs of a repository over the driven ports the composition site passes to it.
pub struct RunsService {
    adapters: RunsAdapters,
}

impl RunsService {
    /// Runs over `adapters`.
    #[must_use]
    pub const fn new(adapters: RunsAdapters) -> Self {
        Self { adapters }
    }

    /// The workspace's files under `prefix`: its base's, with its changes applied.
    async fn files(
        &self,
        repository: RepositoryId,
        draft: &Draft,
        prefix: &Prefix,
    ) -> Result<BTreeMap<Path, File>, RunsError> {
        Ok(workspace_files(&*self.adapters.derived, repository, draft, prefix).await?)
    }

    /// The workspace's run file, where it has one.
    async fn run_file(
        &self,
        repository: RepositoryId,
        draft: &Draft,
    ) -> Result<Option<File>, RunsError> {
        let file = self.adapters.definitions.run_file();
        Ok(self
            .files(repository, draft, &Prefix::from(file.clone()))
            .await?
            .remove(file))
    }

    /// The settings `found`, the content of the run file, holds.
    async fn settings(
        &self,
        repository: RepositoryId,
        found: File,
    ) -> Result<RunSettings, RunsError> {
        let bytes = self.adapters.objects.get(repository, found.content).await?;
        self.adapters
            .definitions
            .parse_run(&bytes)
            .map_err(|error| RunsError::RunFile {
                file: self.adapters.definitions.run_file().clone(),
                error,
            })
    }

    /// The tree each of `caches` starts from. A ref the workspace lacks is seeded from `main`'s,
    /// which needs read on every file of `main`; where `main` has none either, it starts empty
    /// and nothing is asked.
    async fn cache_trees(
        &self,
        caller: &Caller,
        organisation: &crate::domain::name::OrganisationName,
        name: &crate::domain::name::RepositoryName,
        repository: RepositoryId,
        draft: &Draft,
        caches: &BTreeSet<Path>,
    ) -> Result<BTreeMap<Path, crate::domain::address::Address>, RunsError> {
        let mut trees = BTreeMap::new();
        let mut seeds = BTreeMap::new();
        for path in caches {
            if let Some(held) = self
                .adapters
                .cache_refs
                .workspace_cache_ref(repository, draft.instance, path)
                .await?
            {
                trees.insert(path.clone(), held);
                continue;
            }
            let seed = self
                .adapters
                .cache_refs
                .cache_ref(repository, &Prefix::whole(), path)
                .await?
                .map(|main| main.tree);
            seeds.insert(path.clone(), seed);
        }
        if seeds.values().any(Option::is_some)
            && let Some(main) = self.adapters.log.read_ref(repository, Ref::Main).await?
        {
            let root = self.adapters.derived.root(repository, main).await?;
            let tree = tree_files(&*self.adapters.derived, repository, root).await?;
            require_paths(
                &*self.adapters.authorisation,
                caller.principal(),
                organisation,
                name,
                repository,
                Access::Read,
                tree.keys(),
            )
            .await?;
        }
        for (path, seed) in seeds {
            let tree = create_workspace_cache_ref(
                &*self.adapters.cache_refs,
                repository,
                draft.instance,
                &path,
                seed.unwrap_or_else(empty_output_tree),
            )
            .await?;
            trees.insert(path, tree);
        }
        Ok(trees)
    }

    /// The run `run` where `caller` started it in `repository`.
    async fn own(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        run: RunId,
    ) -> Result<Run, RunsError> {
        readable_repository(
            &*self.adapters.directory,
            &*self.adapters.authorisation,
            caller.principal(),
            repository,
        )
        .await?;
        let found = self
            .adapters
            .queue
            .run(run)
            .await
            .map_err(|error| match error {
                JobQueueError::NotFound(_) => RunsError::NotFound(run),
                other @ (JobQueueError::Finished(_) | JobQueueError::Backend(_)) => other.into(),
            })?;
        if found.request.principal != *caller.principal() || found.request.repository != repository
        {
            return Err(RunsError::NotFound(run));
        }
        Ok(found)
    }
}

#[async_trait::async_trait]
impl Runs for RunsService {
    async fn start(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        request: StartRun,
    ) -> Result<Run, RunsError> {
        let (organisation, name) = readable_repository(
            &*self.adapters.directory,
            &*self.adapters.authorisation,
            caller.principal(),
            repository,
        )
        .await?;
        require(
            &*self.adapters.authorisation,
            caller.principal(),
            Action::Run,
            &Resource::Repository {
                organisation: organisation.clone(),
                name: name.clone(),
                repository,
            },
        )
        .await?;
        let StartRun {
            command,
            prefix,
            add,
            outputs,
        } = request;
        let prefix = prefix.unwrap_or_else(Prefix::whole);
        if let Some(outside) = add.iter().find(|path| !prefix.covers(path)) {
            return Err(RunsError::AddOutsidePrefix(outside.clone()));
        }
        let held = self
            .adapters
            .drafts
            .get(
                repository,
                &DraftId {
                    principal: caller.principal().clone(),
                    name: draft.clone(),
                },
            )
            .await?;
        let files = self.files(repository, &held, &prefix).await?;
        let run_file = self.run_file(repository, &held).await?;
        let run_path = run_file
            .as_ref()
            .map(|_| self.adapters.definitions.run_file().clone());
        require_paths(
            &*self.adapters.authorisation,
            caller.principal(),
            &organisation,
            &name,
            repository,
            Access::Read,
            files.keys().chain(run_path.iter()),
        )
        .await?;
        let settings = match run_file {
            Some(found) => self.settings(repository, found).await?,
            None => RunSettings::default(),
        };
        let caches = self
            .cache_trees(
                caller,
                &organisation,
                &name,
                repository,
                &held,
                &settings.caches,
            )
            .await?;
        Ok(self
            .adapters
            .queue
            .queue_run(NewRun {
                organisation,
                repository,
                principal: caller.principal().clone(),
                draft: draft.clone(),
                instance: held.instance,
                version: held.version,
                base: held.base,
                command,
                prefix,
                add,
                outputs,
                labels: settings.labels,
                timeout: settings.timeout,
                files,
                caches,
            })
            .await?)
    }

    async fn get(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        run: RunId,
    ) -> Result<Run, RunsError> {
        self.own(caller, repository, run).await
    }

    async fn latest(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
    ) -> Result<Run, RunsError> {
        readable_repository(
            &*self.adapters.directory,
            &*self.adapters.authorisation,
            caller.principal(),
            repository,
        )
        .await?;
        let held = self
            .adapters
            .drafts
            .get(
                repository,
                &DraftId {
                    principal: caller.principal().clone(),
                    name: draft.clone(),
                },
            )
            .await?;
        self.adapters
            .queue
            .latest_run(repository, caller.principal(), draft, held.instance)
            .await?
            .ok_or_else(|| RunsError::NoRun(draft.clone()))
    }

    async fn cancel(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        run: RunId,
    ) -> Result<Run, RunsError> {
        self.own(caller, repository, run).await?;
        self.adapters
            .queue
            .cancel_run(run)
            .await
            .map_err(|error| match error {
                JobQueueError::Finished(_) => RunsError::Finished(run),
                other @ (JobQueueError::NotFound(_) | JobQueueError::Backend(_)) => other.into(),
            })
    }

    async fn streams(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        run: &Run,
    ) -> Result<StreamDownloads, RunsError> {
        if run.request.principal != *caller.principal() || run.request.repository != repository {
            return Err(RunsError::NotFound(run.id));
        }
        let Some(result) = run.ended() else {
            return Ok(StreamDownloads::default());
        };
        let grant = |stream: Option<RunStream>| async move {
            match stream {
                Some(stream) => Ok(Some(
                    self.adapters
                        .objects
                        .download_grant(repository, stream.address)
                        .await?,
                )),
                None => Ok::<_, RunsError>(None),
            }
        };
        Ok(StreamDownloads {
            stdout: grant(result.stdout).await?,
            stderr: grant(result.stderr).await?,
        })
    }

    async fn files(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        run: RunId,
        prefix: &Prefix,
    ) -> Result<Vec<OutputFile>, RunsError> {
        let held = self.own(caller, repository, run).await?;
        let Some(tree) = held.ended().and_then(|result| result.output_tree) else {
            return Ok(Vec::new());
        };
        let mut files = Vec::new();
        for (path, file) in
            walk_tree::<RunsError>(&*self.adapters.objects, repository, tree).await?
        {
            if prefix.covers(&path) {
                let download = self
                    .adapters
                    .objects
                    .download_grant(repository, file.content)
                    .await?;
                files.push(OutputFile {
                    path,
                    file,
                    download,
                });
            }
        }
        Ok(files)
    }
}
