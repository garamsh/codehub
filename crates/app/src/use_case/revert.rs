//! Reverting a landed changeset whose blocking check failed or timed out (ADR 0035).

use crate::domain::job::Job;
use crate::domain::repository::RepositoryId;

/// Where a blocking check of a landed changeset has failed or timed out, the changeset is to be
/// reverted on `main`: this is the place that does it.
///
/// It reverts nothing yet. It is called wherever a blocking check ends in failure, in a timeout
/// or because its definitions could not be read, and `main` is left as it is.
pub(crate) const fn revert_on_blocking_failure(_repository: RepositoryId, _job: &Job) {}
