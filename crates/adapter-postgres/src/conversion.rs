//! Conversions between the application's values and what the tables store.

use std::collections::BTreeSet;

use app::domain::address::Address;
use app::domain::check::{CheckDefinition, Command, Timeout};
use app::domain::job::{JobId, JobState};
use app::domain::name::Label;
use app::domain::path::{Component, Path, Prefix};
use app::domain::repository::RepositoryId;
use app::domain::revert::Revert;
use app::domain::runner::RegistrationId;
use app::port::driven::membership::{Access, Role};

/// The `bigint` a repository is stored under.
pub(crate) fn repository_column(repository: RepositoryId) -> Result<i64, String> {
    i64::try_from(repository.value())
        .map_err(|_| format!("repository {repository} is outside the stored range"))
}

/// The repository stored as the `bigint` `value`.
pub(crate) fn repository(value: i64) -> Result<RepositoryId, String> {
    unsigned(value, "repository").map(RepositoryId::new)
}

/// The registration stored as the `bigint` `value`.
pub(crate) fn registration(value: i64) -> Result<RegistrationId, String> {
    unsigned(value, "runner registration").map(RegistrationId::new)
}

/// The job stored as the `bigint` `value`.
pub(crate) fn job(value: i64) -> Result<JobId, String> {
    unsigned(value, "job").map(JobId::new)
}

/// The `bigint` a job or a registration is stored under.
pub(crate) fn number_column(value: u64, subject: &str) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("{subject} {value} is outside the stored range"))
}

fn unsigned(value: i64, subject: &str) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("a stored {subject} identifier {value} is negative"))
}

/// The `text[]` of a set of labels.
pub(crate) fn labels_column(labels: &BTreeSet<Label>) -> Vec<String> {
    labels
        .iter()
        .map(|label| label.as_str().to_owned())
        .collect()
}

/// The labels stored as `text[]`.
pub(crate) fn labels(texts: &[String]) -> Result<BTreeSet<Label>, String> {
    texts
        .iter()
        .map(|text| {
            text.parse()
                .map_err(|error| format!("a stored label is invalid: {error}"))
        })
        .collect()
}

/// The columns of a stored check.
pub(crate) struct CheckColumns<'a> {
    pub(crate) name: &'a str,
    pub(crate) command: &'a str,
    pub(crate) labels: &'a [String],
    pub(crate) blocking: bool,
    pub(crate) timeout_seconds: i64,
    pub(crate) folder: &'a [u8],
    pub(crate) inputs: &'a [String],
    pub(crate) needs: &'a [String],
    pub(crate) outputs: &'a [Vec<u8>],
    pub(crate) caches: &'a [Vec<u8>],
}

/// The check stored as its columns.
pub(crate) fn check(columns: &CheckColumns<'_>) -> Result<CheckDefinition, String> {
    let timeout = u64::try_from(columns.timeout_seconds)
        .map_err(|_| format!("a stored timeout {} is negative", columns.timeout_seconds))
        .and_then(|seconds| Timeout::new(seconds).map_err(|error| error.to_string()))?;
    Ok(CheckDefinition {
        folder: prefix(columns.folder)?,
        name: columns
            .name
            .parse()
            .map_err(|error| format!("a stored check name is invalid: {error}"))?,
        command: Command::new(columns.command).map_err(|error| error.to_string())?,
        labels: labels(columns.labels)?,
        blocking: columns.blocking,
        timeout,
        inputs: parsed(columns.inputs, "input")?,
        needs: parsed(columns.needs, "need")?,
        outputs: columns
            .outputs
            .iter()
            .map(|bytes| path(bytes))
            .collect::<Result<_, _>>()?,
        caches: columns
            .caches
            .iter()
            .map(|bytes| path(bytes))
            .collect::<Result<_, _>>()?,
    })
}

/// The `text[]` of a check's inputs or needs, as the check file gives them.
pub(crate) fn texts<T: ToString>(values: &[T]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

/// The `bytea[]` of a check's outputs or caches.
pub(crate) fn path_columns(paths: &[Path]) -> Vec<Vec<u8>> {
    paths.iter().map(path_column).collect()
}

fn parsed<T: std::str::FromStr>(texts: &[String], subject: &str) -> Result<Vec<T>, String> {
    texts
        .iter()
        .map(|text| {
            text.parse()
                .map_err(|_| format!("a stored check {subject} is invalid: {text:?}"))
        })
        .collect()
}

/// The `state` column of `state`.
pub(crate) const fn state_column(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Running { .. } => "running",
        JobState::Passed => "passed",
        JobState::Failed => "failed",
        JobState::TimedOut => "timed_out",
        JobState::Cancelled => "cancelled",
    }
}

/// The state stored as `text`, with the lease and deadline columns a running job holds.
pub(crate) fn state(
    text: &str,
    lease_expires_at: Option<i64>,
    deadline: Option<i64>,
) -> Result<JobState, String> {
    match text {
        "queued" => Ok(JobState::Queued),
        "running" => lease_expires_at
            .zip(deadline)
            .map(|(lease_expires_at, deadline)| JobState::Running {
                lease_expires_at,
                deadline,
            })
            .ok_or_else(|| "a stored running job has no lease or deadline".to_owned()),
        "passed" => Ok(JobState::Passed),
        "failed" => Ok(JobState::Failed),
        "timed_out" => Ok(JobState::TimedOut),
        "cancelled" => Ok(JobState::Cancelled),
        other => Err(format!("a stored job state is unknown: {other:?}")),
    }
}

/// The three columns of a revert record, of which at most one is set.
#[derive(Default)]
pub(crate) struct RevertColumns {
    /// The changeset the revert landed as.
    pub(crate) landed: Option<Vec<u8>>,
    /// The paths the revert conflicted at.
    pub(crate) conflicts: Option<Vec<Vec<u8>>>,
    /// Why the revert failed.
    pub(crate) failed: Option<String>,
}

/// The columns of a revert record: the landed changeset, the conflicting paths, or the reason.
pub(crate) fn revert_columns(revert: &Revert) -> RevertColumns {
    match revert {
        Revert::Landed(address) => RevertColumns {
            landed: Some(address.digest().to_vec()),
            ..RevertColumns::default()
        },
        Revert::Conflicted(paths) => RevertColumns {
            conflicts: Some(paths.iter().map(path_column).collect()),
            ..RevertColumns::default()
        },
        Revert::Failed { reason } => RevertColumns {
            failed: Some(reason.clone()),
            ..RevertColumns::default()
        },
    }
}

/// The revert record stored in its columns; none set means no revert was attempted.
pub(crate) fn revert(columns: RevertColumns) -> Result<Option<Revert>, String> {
    match (columns.landed, columns.conflicts, columns.failed) {
        (None, None, None) => Ok(None),
        (Some(bytes), None, None) => address(bytes).map(Revert::Landed).map(Some),
        (None, Some(paths), None) => paths
            .iter()
            .map(|bytes| path(bytes))
            .collect::<Result<_, _>>()
            .map(Revert::Conflicted)
            .map(Some),
        (None, None, Some(reason)) => Ok(Some(Revert::Failed { reason })),
        (Some(_), _, _) | (_, Some(_), _) => {
            Err("a stored revert is more than one of landed, conflicted and failed".to_owned())
        }
    }
}

/// The address whose digest is the `bytea` `bytes`.
pub(crate) fn address(bytes: Vec<u8>) -> Result<Address, String> {
    <[u8; 32]>::try_from(bytes)
        .map(Address::from_digest)
        .map_err(|bytes| format!("a stored address has {} bytes, not 32", bytes.len()))
}

/// The `bytea` of a path: its components joined by `/`.
pub(crate) fn path_column(path: &Path) -> Vec<u8> {
    join(path.components())
}

/// The path stored as `bytes`.
pub(crate) fn path(bytes: &[u8]) -> Result<Path, String> {
    let components = split(bytes)?;
    Path::new(components).map_err(|error| format!("a stored path is invalid: {error}"))
}

/// The `bytea` of a prefix: its components joined by `/`; empty for the whole repository.
pub(crate) fn prefix_column(prefix: &Prefix) -> Vec<u8> {
    join(prefix.components())
}

/// The prefix stored as `bytes`.
pub(crate) fn prefix(bytes: &[u8]) -> Result<Prefix, String> {
    if bytes.is_empty() {
        Ok(Prefix::whole())
    } else {
        split(bytes).map(Prefix::new)
    }
}

/// The `role` column of `role`.
pub(crate) const fn role_column(role: Role) -> &'static str {
    match role {
        Role::Admin => "admin",
        Role::Member => "member",
    }
}

/// The role stored as `text`.
pub(crate) fn role(text: &str) -> Result<Role, String> {
    match text {
        "admin" => Ok(Role::Admin),
        "member" => Ok(Role::Member),
        other => Err(format!("a stored role is unknown: {other:?}")),
    }
}

/// The `access` column of `access`.
pub(crate) const fn access_column(access: Access) -> &'static str {
    match access {
        Access::Read => "read",
        Access::Write => "write",
    }
}

/// The access stored as `text`.
pub(crate) fn access(text: &str) -> Result<Access, String> {
    match text {
        "read" => Ok(Access::Read),
        "write" => Ok(Access::Write),
        other => Err(format!("a stored access is unknown: {other:?}")),
    }
}

fn join(components: &[Component]) -> Vec<u8> {
    components
        .iter()
        .map(Component::as_bytes)
        .collect::<Vec<_>>()
        .join(&b'/')
}

fn split(bytes: &[u8]) -> Result<Vec<Component>, String> {
    bytes
        .split(|byte| *byte == b'/')
        .map(|part| {
            Component::new(part.to_vec())
                .map_err(|error| format!("a stored component is invalid: {error}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_and_a_prefix_round_trip_through_their_columns() {
        let path: Path = "src/a b/main.rs".parse().unwrap();
        assert_eq!(path_column(&path), b"src/a b/main.rs");
        assert_eq!(super::path(&path_column(&path)), Ok(path.clone()));
        let prefix = Prefix::from(path);
        assert_eq!(super::prefix(&prefix_column(&prefix)), Ok(prefix));
        assert_eq!(prefix_column(&Prefix::whole()), Vec::<u8>::new());
        assert_eq!(super::prefix(b""), Ok(Prefix::whole()));
    }

    #[test]
    fn a_role_and_an_access_round_trip_through_their_columns() {
        for held in [Role::Admin, Role::Member] {
            assert_eq!(role(role_column(held)), Ok(held));
        }
        for held in [Access::Read, Access::Write] {
            assert_eq!(access(access_column(held)), Ok(held));
        }
        assert!(role("owner").is_err());
        assert!(access("admin").is_err());
    }

    #[test]
    fn an_address_round_trips_and_a_short_column_is_refused() {
        let address_of = Address::of(b"x");
        assert_eq!(address(address_of.digest().to_vec()), Ok(address_of));
        assert!(address(vec![0; 31]).is_err());
    }

    #[test]
    fn a_job_state_round_trips_through_its_columns() {
        let running = JobState::Running {
            lease_expires_at: 30,
            deadline: 600,
        };
        for held in [
            JobState::Queued,
            running,
            JobState::Passed,
            JobState::Failed,
            JobState::TimedOut,
            JobState::Cancelled,
        ] {
            let (lease, deadline) = match held {
                JobState::Running {
                    lease_expires_at,
                    deadline,
                } => (Some(lease_expires_at), Some(deadline)),
                JobState::Queued
                | JobState::Passed
                | JobState::Failed
                | JobState::TimedOut
                | JobState::Cancelled => (None, None),
            };
            assert_eq!(state(state_column(held), lease, deadline), Ok(held));
        }
        assert!(state("running", None, None).is_err());
        assert!(state("lost", None, None).is_err());
    }

    #[test]
    fn labels_round_trip_and_a_stored_label_that_is_not_one_is_refused() {
        let offered: BTreeSet<Label> = ["linux", "gpu"]
            .iter()
            .map(|label| label.parse().unwrap())
            .collect();
        assert_eq!(labels_column(&offered), ["gpu", "linux"]);
        assert_eq!(labels(&labels_column(&offered)), Ok(offered));
        assert!(labels(&["Not A Label".to_owned()]).is_err());
    }

    #[test]
    fn a_negative_identifier_is_refused_and_one_beyond_the_range_is_not_stored() {
        assert_eq!(job(7), Ok(JobId::new(7)));
        assert!(job(-1).is_err());
        assert!(registration(-1).is_err());
        assert!(repository(-1).is_err());
        assert_eq!(number_column(7, "job"), Ok(7));
        assert!(number_column(u64::MAX, "job").is_err());
    }

    #[test]
    fn a_revert_round_trips_through_its_columns() {
        let landed = Revert::Landed(Address::from_digest([7; 32]));
        assert_eq!(revert(revert_columns(&landed)), Ok(Some(landed)));
        let paths: BTreeSet<Path> = ["a/b", "c"]
            .iter()
            .map(|text| text.parse().unwrap())
            .collect();
        let conflicted = Revert::Conflicted(paths);
        assert_eq!(revert(revert_columns(&conflicted)), Ok(Some(conflicted)));
        let failed = Revert::Failed {
            reason: "job queue failed".to_owned(),
        };
        assert_eq!(revert(revert_columns(&failed)), Ok(Some(failed)));
        assert_eq!(revert(RevertColumns::default()), Ok(None));
        assert!(
            revert(RevertColumns {
                landed: Some(vec![0; 32]),
                failed: Some(String::new()),
                ..RevertColumns::default()
            })
            .is_err()
        );
    }

    #[test]
    fn a_repository_beyond_the_stored_range_is_refused() {
        assert_eq!(repository_column(RepositoryId::new(7)), Ok(7));
        assert!(repository_column(RepositoryId::new(u64::MAX)).is_err());
    }
}
