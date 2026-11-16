//! A compare-and-swap whose connection fails after the statement was sent: the backend that runs
//! it is ended, and the swap is answered by whether its changeset is on `main`.

#[cfg(test)]
pub mod common;

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

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
use sqlx::{Connection, PgConnection};
use test_support::contract::{Deployment, MAX_UPLOAD_SIZE, draft};
use tokio::time::timeout;

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
async fn through_a_proxy(harness: &Harness) -> (Proxy, Arc<PostgresChangesetLog>) {
    let url = harness.url();
    let options = PgConnectOptions::from_str(&url).unwrap();
    let upstream = format!("{}:{}", options.get_host(), options.get_port());
    let proxy = Proxy::start(upstream.clone()).await;
    let database = Database::connect(&proxy.url(&url, &upstream), 2)
        .await
        .unwrap();
    (proxy, Arc::new(PostgresChangesetLog::new(&database)))
}

/// How long a step that waits for another may take.
#[cfg(test)]
const BOUND: Duration = Duration::from_secs(30);

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
            log,
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

#[tokio::test]
async fn a_won_swap_followed_by_another_move_of_main_is_answered_as_won() {
    let harness = Harness::new().await;
    let (proxy, log) = through_a_proxy(&harness).await;
    let (repository, first, second) = main_at_first_of_two(&harness, &log).await;
    let other = log
        .append(repository, changeset(Some(second), 3))
        .await
        .unwrap();

    proxy.arm(Fault::HeldReply(b"UPDATE 1\0"));
    let swap = tokio::spawn({
        let log = log.clone();
        async move {
            log.compare_and_swap(repository, Ref::Main, Some(first), second)
                .await
        }
    });
    timeout(BOUND, proxy.held()).await.unwrap();
    // The swap committed, and another replica moves `main` on before the client learns that its
    // connection failed.
    harness
        .log()
        .compare_and_swap(repository, Ref::Main, Some(second), other)
        .await
        .unwrap();
    proxy.release();

    assert_eq!(timeout(BOUND, swap).await.unwrap().unwrap(), Ok(()));
    let main = harness.log().read_ref(repository, Ref::Main).await.unwrap();
    assert_eq!(main, Some(other));
}

/// How many backends other than the caller's run a statement that holds `text`, and are not idle.
#[cfg(test)]
async fn running(connection: &mut PgConnection, text: &str) -> i64 {
    sqlx::query_scalar(
        "select count(*) from pg_stat_activity \
         where datname = current_database() and pid <> pg_backend_pid() and state <> 'idle' \
           and position($1 in query) > 0",
    )
    .bind(text)
    .fetch_one(connection)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_swap_whose_backend_still_waits_when_the_connection_fails_is_answered_as_main_ends_up() {
    let harness = Harness::new().await;
    let (proxy, log) = through_a_proxy(&harness).await;
    let (repository, first, second) = main_at_first_of_two(&harness, &log).await;
    // Another session holds the rows of `refs`, so the statement of the swap waits for them.
    let mut locker = PgConnection::connect(&harness.url()).await.unwrap();
    let mut hold = locker.begin().await.unwrap();
    sqlx::query("select 1 from refs for update")
        .execute(&mut *hold)
        .await
        .unwrap();

    // The text of a statement and its parameters travel in separate messages; the parameters hold
    // the digest of the new changeset.
    let new = Box::leak(second.digest().as_slice().to_vec().into_boxed_slice());
    proxy.arm(Fault::HeldRequest(new));
    let swap = tokio::spawn({
        let log = log.clone();
        async move {
            log.compare_and_swap(repository, Ref::Main, Some(first), second)
                .await
        }
    });
    timeout(BOUND, proxy.held()).await.unwrap();
    // The server got the statement and waits for the lock; now the link drops.
    timeout(BOUND, async {
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "select count(*) from pg_stat_activity \
                 where datname = current_database() and pid <> pg_backend_pid() \
                   and wait_event_type = 'Lock' and position('update refs' in query) > 0",
            )
            .fetch_one(&mut *hold)
            .await
            .unwrap();
            if waiting > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    proxy.release();
    let swapped = timeout(BOUND, swap).await.unwrap().unwrap();
    // Only now does the lock go, so a backend that was still alive would commit.
    hold.rollback().await.unwrap();
    timeout(BOUND, async {
        while running(&mut locker, "update refs").await > 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let main = harness.log().read_ref(repository, Ref::Main).await.unwrap();
    assert!(matches!(swapped, Err(ChangesetLogError::Backend(_))));
    assert_eq!(
        main,
        Some(first),
        "the answer is not applied, and main agrees"
    );
}
