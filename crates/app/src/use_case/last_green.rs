//! Moving `last-green`: forward along `main` once every blocking check of a changeset has
//! passed (ADR 0035).

use crate::domain::address::Address;
use crate::domain::job::Job;
use crate::domain::repository::RepositoryId;
use crate::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};

/// Whether every blocking job of `jobs` has passed, by a run or from the cache; true where there
/// is none. A skipped job has not passed.
pub(crate) fn blocking_checks_passed(jobs: &[Job]) -> bool {
    jobs.iter()
        .filter(|job| job.check.blocking)
        .all(|job| job.state.is_passed())
}

/// Points `last-green` of `repository` at `candidate`, unless it already points at `candidate`
/// or at a descendant of it, following first parents. Where `last-green` moves between the read
/// and the compare-and-swap, decides again over where it moved.
///
/// # Errors
///
/// The failure of the changeset log.
pub(crate) async fn advance<L: ChangesetLog>(
    log: &L,
    repository: RepositoryId,
    candidate: Address,
) -> Result<(), ChangesetLogError> {
    loop {
        let current = log.read_ref(repository, Ref::LastGreen).await?;
        if let Some(current) = current
            && !descends(log, repository, candidate, current).await?
        {
            return Ok(());
        }
        match log
            .compare_and_swap(repository, Ref::LastGreen, current, candidate)
            .await
        {
            Ok(()) => return Ok(()),
            Err(ChangesetLogError::RefMoved { .. }) => {}
            Err(error) => return Err(error),
        }
    }
}

/// Whether `candidate` is a strict first-parent descendant of `current`.
async fn descends<L: ChangesetLog>(
    log: &L,
    repository: RepositoryId,
    candidate: Address,
    current: Address,
) -> Result<bool, ChangesetLogError> {
    let mut at = candidate;
    while at != current {
        match log.get(repository, at).await?.parents.first() {
            Some(parent) => at = *parent,
            None => return Ok(false),
        }
    }
    Ok(candidate != current)
}
