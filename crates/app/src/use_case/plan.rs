//! The plan of a draft: what landing it now would do about each check, made from the same keys
//! the post-landing queueing computes and writing nothing.

use std::collections::{BTreeMap, BTreeSet};

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::check::{CheckDefinition, CheckId, CheckKey};
use crate::domain::draft::DraftId;
use crate::domain::path::Path;
use crate::domain::repository::RepositoryId;
use crate::port::driven::changeset_log::{ChangesetLog, Ref};
use crate::port::driven::check_definitions::CheckDefinitions;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::draft_store::DraftStore;
use crate::port::driven::job_queue::JobQueue;
use crate::port::driven::object_store::ObjectStore;
use crate::port::driving::checks::{Plan, PlanError, PlanOutcome, PlannedCheck, RunReason};
use crate::use_case::land::rebase;
use crate::use_case::tree_checks::{DefinitionsError, Unreadable, read_definitions, tree_files};

/// The driven ports a plan is made over.
pub(crate) struct PlanPorts<'a, Q, L, O, D, C, S> {
    pub queue: &'a Q,
    pub log: &'a L,
    pub objects: &'a O,
    pub derived: &'a D,
    pub definitions: &'a C,
    pub drafts: &'a S,
}

/// What a check that is planned left for the checks that need it: its key and output tree where
/// it is cached.
type Cached = Option<(CheckKey, Address)>;

/// The plan of the draft `draft` of `repository`: the draft rebased onto `main` as landing would,
/// its definitions read from the resulting tree, and each check's key looked up.
///
/// A check whose needs all have keys has its own key computed from its input files and the keys
/// and outputs of its needs, as queueing a landing computes it. A check one of whose needs would
/// run has no key yet and would run.
pub(crate) async fn plan<Q, L, O, D, C, S>(
    ports: &PlanPorts<'_, Q, L, O, D, C, S>,
    repository: RepositoryId,
    draft: &DraftId,
) -> Result<Plan, PlanError>
where
    Q: JobQueue + Sync,
    L: ChangesetLog + Sync,
    O: ObjectStore + Sync,
    D: DerivedData + Sync,
    C: CheckDefinitions + Sync,
    S: DraftStore + Sync,
{
    let staged = ports.drafts.get(repository, draft).await?;
    let main = ports.log.read_ref(repository, Ref::Main).await?;
    let before = match main {
        Some(main) => {
            let root = ports.derived.root(repository, main).await?;
            tree_files(ports.derived, repository, root).await?
        }
        None => BTreeMap::new(),
    };
    let changes = rebase(
        ports.objects,
        ports.log,
        ports.derived,
        repository,
        staged.base,
        main,
        staged.changes,
    )
    .await?;

    let mut after = before.clone();
    for (path, change) in changes {
        match change {
            Change::File(file) => after.insert(path, file.content),
            Change::Deletion => after.remove(&path),
        };
    }
    let changed: BTreeSet<&Path> = before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .collect();

    let definitions = read_definitions(ports.objects, ports.definitions, repository, &after)
        .await
        .map_err(|error| match error {
            DefinitionsError::Objects(error) => PlanError::Objects(error),
            DefinitionsError::Unreadable(Unreadable { folder, reason }) => PlanError::Definitions {
                file: CheckDefinition::unreadable(&folder, ports.definitions.file())
                    .command
                    .as_str()
                    .to_owned(),
                reason,
            },
        })?;
    let mut checks = Vec::new();
    let mut settled: BTreeMap<CheckId, Cached> = BTreeMap::new();
    let mut remaining: BTreeMap<CheckId, &CheckDefinition> = definitions
        .iter()
        .map(|definition| (definition.id(), definition))
        .collect();
    while let Some(id) = remaining
        .iter()
        .find(|(_, definition)| definition.needed().all(|need| settled.contains_key(&need)))
        .map(|(id, _)| id.clone())
    {
        let Some(definition) = remaining.remove(&id) else {
            break;
        };
        let inputs_changed = changed.iter().any(|path| definition.reads(path));
        let mut reasons = Vec::new();
        if inputs_changed {
            reasons.push(RunReason::InputsChanged);
        }
        let mut needed = Vec::new();
        for need in definition.needed() {
            match settled.get(&need) {
                Some(Some(result)) => needed.push(*result),
                Some(None) | None => reasons.push(RunReason::NeedRuns(need)),
            }
        }
        let mut cached = None;
        if needed.len() == definition.needs.len() {
            let key = CheckKey::of_tree(definition, &after, &needed);
            cached = ports
                .queue
                .passed_with_key(repository, key)
                .await?
                .map(|output| (key, output));
            if cached.is_none() && reasons.is_empty() {
                reasons.push(RunReason::NoPassedRun);
            }
        }
        let outcome = if cached.is_some() {
            PlanOutcome::Cached
        } else {
            PlanOutcome::Run(reasons)
        };
        settled.insert(id.clone(), cached);
        checks.push(PlannedCheck {
            check: id,
            blocking: definition.blocking,
            inputs: definition.inputs.clone(),
            outcome,
        });
    }
    Ok(Plan { checks })
}
