//! Moving a pointer forward along `main`: the first-parent descent and the compare-and-swap loop
//! that `last-green` and the cache refs share.

use std::future::Future;

use crate::domain::address::Address;
use crate::domain::repository::RepositoryId;
use crate::port::driven::changeset_log::{ChangesetLog, ChangesetLogError};

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

/// Points a pointer at `candidate`, unless it already points at `candidate` or at a descendant of
/// it, following first parents. `read` returns the changeset the pointer is at, and `swap`
/// points it at `candidate` where it is still at the changeset given, and returns whether it was.
/// Where the pointer moves between the read and the swap, decides again over where it moved.
///
/// # Errors
///
/// The failure of the changeset log, or of `read` or `swap`.
pub(crate) async fn advance_forward<L, E, R, RF, S, SF>(
    log: &L,
    repository: RepositoryId,
    candidate: Address,
    read: R,
    swap: S,
) -> Result<(), E>
where
    L: ChangesetLog,
    E: From<ChangesetLogError>,
    R: Fn() -> RF,
    RF: Future<Output = Result<Option<Address>, E>>,
    S: Fn(Option<Address>) -> SF,
    SF: Future<Output = Result<bool, E>>,
{
    loop {
        let current = read().await?;
        if let Some(current) = current
            && !descends(log, repository, candidate, current).await?
        {
            return Ok(());
        }
        if swap(current).await? {
            return Ok(());
        }
    }
}
