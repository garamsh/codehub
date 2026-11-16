//! A compare-and-swap whose connection fails after the statement was sent: the ref is read again,
//! and the swap is answered as the ref shows.

#[cfg(test)]
pub mod common;

use std::collections::BTreeMap;
use std::str::FromStr;

use adapter_postgres::{Database, PostgresChangesetLog};
use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::repository::RepositoryId;
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use app::port::driving::source_control::{Landing, SourceControl};
use app::use_case::source_control::{SourceControlAdapters, SourceControlService};
use common::Harness;
use common::proxy::{Fault, Proxy};
use sqlx::postgres::PgConnectOptions;
use test_support::contract::{Deployment, MAX_UPLOAD_SIZE, draft};

/// A changeset whose parent is `parent`, told apart from its siblings by `n`.
#[cfg(test)]
fn changeset(parent: Option<Address>, n: i64) -> Changeset {
    Changeset {
        parents: parent.into_iter().collect(),
        author: "ada".to_owned(),
        timestamp: n,
        message: format!("changeset {n}"),
        changes: BTreeMap::new(),
    }
}

/// A changeset log over a pool whose connections go through a proxy to the harness's database.
#[cfg(test)]
async fn through_a_proxy(harness: &Harness) -> (Proxy, PostgresChangesetLog) {
    let url = harness.url();
    let options = PgConnectOptions::from_str(&url).unwrap();
    let upstream = format!("{}:{}", options.get_host(), options.get_port());
    let proxy = Proxy::start(upstream.clone()).await;
    let database = Database::connect(&proxy.url(&url, &upstream), 2)
        .await
        .unwrap();
    (proxy, PostgresChangesetLog::new(&database))
}

/// A repository, `main` of which points at the first of the two changesets that follow, which are
/// appended; the proxy has no fault armed.
#[cfg(test)]
async fn main_at_first_of_two(
    harness: &Harness,
    log: &PostgresChangesetLog,
) -> (RepositoryId, Address, Address) {
    let ada = harness.caller("ada").await;
    let repository = harness.repository(&ada, "acme").await.id;
    let first = log.append(repository, changeset(None, 1)).await.unwrap();
    let second = log
        .append(repository, changeset(Some(first), 2))
        .await
        .unwrap();
    harness
        .log()
        .compare_and_swap(repository, Ref::Main, None, first)
        .await
        .unwrap();
    (repository, first, second)
}

#[tokio::test]
async fn a_swap_that_committed_before_the_connection_failed_is_answered_as_won() {
    let harness = Harness::new().await;
    let (proxy, log) = through_a_proxy(&harness).await;
    let (repository, first, second) = main_at_first_of_two(&harness, &log).await;

    proxy.arm(Fault::Reply(b"UPDATE 1\0"));
    let swapped = log
        .compare_and_swap(repository, Ref::Main, Some(first), second)
        .await;

    assert_eq!(swapped, Ok(()));
    assert!(proxy.spent(), "the connection was failed");
    let main = harness.log().read_ref(repository, Ref::Main).await.unwrap();
    assert_eq!(main, Some(second));
}

#[tokio::test]
async fn a_first_swap_that_committed_before_the_connection_failed_is_answered_as_won() {
    let harness = Harness::new().await;
    let (proxy, log) = through_a_proxy(&harness).await;
    let ada = harness.caller("ada").await;
    let repository = harness.repository(&ada, "acme").await.id;
    let first = log.append(repository, changeset(None, 1)).await.unwrap();

    proxy.arm(Fault::Reply(b"INSERT 0 1\0"));
    let swapped = log
        .compare_and_swap(repository, Ref::Main, None, first)
        .await;

    assert_eq!(swapped, Ok(()));
    assert!(proxy.spent(), "the connection was failed");
    let main = harness.log().read_ref(repository, Ref::Main).await.unwrap();
    assert_eq!(main, Some(first));
}

#[tokio::test]
async fn a_swap_that_never_reached_the_server_is_not_applied_and_is_an_error() {
    let harness = Harness::new().await;
    let (proxy, log) = through_a_proxy(&harness).await;
    let (repository, first, second) = main_at_first_of_two(&harness, &log).await;

    proxy.arm(Fault::Request(b"update refs"));
    let swapped = log
        .compare_and_swap(repository, Ref::Main, Some(first), second)
        .await;

    assert!(matches!(swapped, Err(ChangesetLogError::Backend(_))));
    assert!(proxy.spent(), "the connection was failed");
    let main = harness.log().read_ref(repository, Ref::Main).await.unwrap();
    assert_eq!(main, Some(first));
}

#[tokio::test]
async fn a_swap_that_never_reached_the_server_finds_main_moved_by_another() {
    let harness = Harness::new().await;
    let (proxy, log) = through_a_proxy(&harness).await;
    let (repository, first, second) = main_at_first_of_two(&harness, &log).await;
    let other = log
        .append(repository, changeset(Some(first), 3))
        .await
        .unwrap();
    harness
        .log()
        .compare_and_swap(repository, Ref::Main, Some(first), other)
        .await
        .unwrap();

    proxy.arm(Fault::Request(b"update refs"));
    let swapped = log
        .compare_and_swap(repository, Ref::Main, Some(first), second)
        .await;

    assert_eq!(
        swapped,
        Err(ChangesetLogError::RefMoved {
            current: Some(other)
        })
    );
    assert!(proxy.spent(), "the connection was failed");
}

#[tokio::test]
async fn a_landing_whose_swap_committed_before_the_connection_failed_is_answered_as_landed() {
    let harness = Harness::new().await;
    let (proxy, log) = through_a_proxy(&harness).await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let (seed, late) = (draft("seed"), draft("late"));
    repo.stage(&ada, &seed, &[("a", "a0\n")]).await;
    repo.land(&ada, &seed, "seed").await.unwrap();
    repo.stage(&ada, &late, &[("b", "b1\n")]).await;
    let service = SourceControlService::new(
        SourceControlAdapters {
            objects: harness.objects().clone(),
            log: std::sync::Arc::new(log),
            derived: harness.derived().clone(),
            drafts: harness.drafts().clone(),
            cache_refs: harness.cache_refs().clone(),
            directory: harness.directory().clone(),
            authorisation: harness.authorisation().clone(),
        },
        MAX_UPLOAD_SIZE,
    );

    proxy.arm(Fault::Reply(b"UPDATE 1\0"));
    let landing = Landing {
        message: "late".to_owned(),
        timestamp: 0,
    };
    let landed = service.land(&ada, repo.id, &late, landing).await.unwrap();

    assert!(proxy.spent(), "the connection was failed");
    assert_eq!(repo.main().await, Some(landed));
    assert_eq!(repo.read("b").await.as_deref(), Some("b1\n"));
}
