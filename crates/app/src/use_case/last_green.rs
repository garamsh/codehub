//! Moving `last-green`: forward along `main` once every blocking check of a changeset has
//! passed (ADR 0035).

use crate::domain::address::Address;
use crate::domain::job::Job;
use crate::domain::repository::RepositoryId;
use crate::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use crate::use_case::first_parent::advance_forward;

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
pub(crate) async fn advance(
    log: &dyn ChangesetLog,
    repository: RepositoryId,
    candidate: Address,
) -> Result<(), ChangesetLogError> {
    advance_forward(
        log,
        repository,
        candidate,
        || log.read_ref(repository, Ref::LastGreen),
        |current| async move {
            match log
                .compare_and_swap(repository, Ref::LastGreen, current, candidate)
                .await
            {
                Ok(()) => Ok(true),
                Err(ChangesetLogError::RefMoved { .. }) => Ok(false),
                Err(error) => Err(error),
            }
        },
    )
    .await
}
