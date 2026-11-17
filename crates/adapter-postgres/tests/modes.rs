//! The operating modes of ADR 0054 one database shows: the host that is the writable primary, the
//! read replica that serves immutable reads only, the retry of a statement a connection failure
//! interrupted, and the claim that opens no transaction when nothing waits.

#[cfg(test)]
pub mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::time::Duration;

use adapter_postgres::{
    Database, DatabaseError, PostgresChangesetLog, PostgresJobQueue, PostgresOptions,
};
use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::name::Label;
use app::domain::repository::RepositoryId;
use app::domain::runner::{NewRegistration, Registration};
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use app::port::driven::job_queue::JobQueue;
use app::port::driven::runner_registry::RunnerRegistry;
use common::Harness;
use common::proxy::{Fault, Proxy};
use sqlx::postgres::PgConnectOptions;
use sqlx::{Connection, Executor, PgConnection};
use test_support::contract::Deployment;

/// A changeset with no parents, told apart from its siblings by `n`.
#[cfg(test)]
fn changeset(n: i64) -> Changeset {
    Changeset {
        parents: Vec::new(),
        author: "ada".to_owned(),
        timestamp: n,
        message: format!("changeset {n}"),
        changes: BTreeMap::new(),
    }
}

/// The options of a pool over `url`, with the settings the tests do not vary.
#[cfg(test)]
fn options<'a>(url: &'a str, read_url: Option<&'a str>) -> PostgresOptions<'a> {
    PostgresOptions {
        url,
        pool_size: 3,
        acquire_timeout: Duration::from_secs(2),
        statement_timeout: Duration::ZERO,
        statement_cache: 100,
        read_url,
    }
}

/// `url` with the host `127.0.0.1:1`, where nothing listens, named before its own.
#[cfg(test)]
fn after_a_dead_host(url: &str) -> String {
    let (userinfo, rest) = url.split_once('@').unwrap();
    format!("{userinfo}@127.0.0.1:1,{rest}")
}

/// A repository `acme/app` of `harness`.
#[cfg(test)]
async fn repository(harness: &Harness) -> RepositoryId {
    let ada = harness.caller("ada").await;
    harness.repository(&ada, "acme").await.id
}

#[tokio::test]
async fn a_pool_connects_to_the_first_host_that_is_a_writable_primary() {
    let harness = Harness::new().await;
    let repository = repository(&harness).await;
    let url = after_a_dead_host(&harness.url());

    let database = Database::open(options(&url, None)).await.unwrap();

    assert!(database.is_writable().await.unwrap());
    let log = PostgresChangesetLog::new(&database);
    let id = log.append(repository, changeset(1)).await.unwrap();
    assert_eq!(log.get(repository, id).await.unwrap(), changeset(1));
}

#[tokio::test]
async fn a_pool_is_refused_where_no_host_is_a_writable_primary() {
    let refused = Database::open(options("postgres://u:p@127.0.0.1:1,127.0.0.1:2/db", None))
        .await
        .err()
        .expect("no host answers");
    let DatabaseError::Connect(description) = refused else {
        panic!("not a connection error: {refused}");
    };
    assert!(description.contains("no writable primary"), "{description}");

    let other = Database::open(options(
        "postgres://u:p@a,b/db?target_session_attrs=any",
        None,
    ))
    .await
    .err()
    .expect("only read-write is supported");
    assert!(other.to_string().contains("read-write"), "{other}");
}

/// A role that may read `changesets` of the database at `url` and nothing else, and the URL it
/// connects with.
#[cfg(test)]
async fn reader_of_changesets(url: &str) -> String {
    const ROLE: &str = "codehub_test_replica_reader";
    let mut admin = PgConnection::connect(url).await.unwrap();
    admin
        .execute(
            "do $$ begin create role codehub_test_replica_reader login password 'reader'; \
             exception when duplicate_object then null; end $$",
        )
        .await
        .unwrap();
    admin
        .execute("grant select on changesets to codehub_test_replica_reader")
        .await
        .unwrap();
    let (_, rest) = url.split_once('@').unwrap();
    format!("postgres://{ROLE}:reader@{rest}")
}

/// How many connections of `role` the server has open.
#[cfg(test)]
async fn connections_of(url: &str, role: &str) -> i64 {
    let mut admin = PgConnection::connect(url).await.unwrap();
    sqlx::query_scalar("select count(*) from pg_stat_activity where usename = $1")
        .bind(role)
        .fetch_one(&mut admin)
        .await
        .unwrap()
}

#[tokio::test]
async fn immutable_reads_are_served_by_the_replica_and_refs_never_reach_it() {
    let primary = Harness::new().await;
    let replica = Harness::new().await;
    let repository = repository(&primary).await;
    assert_eq!(
        self::repository(&replica).await,
        repository,
        "both databases number their first repository alike"
    );
    // A role that may read nothing but changesets: a ref read sent to the replica fails.
    let reader = reader_of_changesets(&replica.url()).await;
    let database = Database::open(options(&primary.url(), Some(&reader)))
        .await
        .unwrap();
    let log = PostgresChangesetLog::new(&database);

    // Only the replica holds `seen`, only the primary `kept`, neither `absent`.
    let seen = replica
        .log()
        .append(repository, changeset(1))
        .await
        .unwrap();
    let kept = primary
        .log()
        .append(repository, changeset(2))
        .await
        .unwrap();
    let absent = Address::of(b"absent");

    // The primary's main and the replica's differ, and the swap and the read answer by the
    // primary's alone. No connection of the reader exists after them: the replica pool is made
    // when it is first used.
    replica
        .log()
        .compare_and_swap(repository, Ref::Main, None, seen)
        .await
        .unwrap();
    log.compare_and_swap(repository, Ref::Main, None, kept)
        .await
        .unwrap();
    assert_eq!(
        log.read_ref(repository, Ref::Main).await.unwrap(),
        Some(kept)
    );
    assert_eq!(
        connections_of(&primary.url(), "codehub_test_replica_reader").await,
        0
    );

    assert_eq!(log.get(repository, seen).await.unwrap(), changeset(1));
    assert!(connections_of(&primary.url(), "codehub_test_replica_reader").await > 0);
    assert_eq!(log.get(repository, kept).await.unwrap(), changeset(2));
    assert_eq!(
        log.get(repository, absent).await,
        Err(ChangesetLogError::NotFound(absent))
    );
}

#[tokio::test]
async fn an_immutable_read_falls_back_to_the_primary_where_the_replica_does_not_answer() {
    let harness = Harness::new().await;
    let repository = repository(&harness).await;
    let database = Database::open(options(
        &harness.url(),
        Some("postgres://u:p@127.0.0.1:1/db"),
    ))
    .await
    .unwrap();
    let log = PostgresChangesetLog::new(&database);
    let id = log.append(repository, changeset(1)).await.unwrap();

    assert_eq!(log.get(repository, id).await.unwrap(), changeset(1));
}

/// A changeset log over a pool whose connections go through `proxy` to the harness's database.
#[cfg(test)]
async fn through_a_proxy(harness: &Harness) -> (Proxy, Database) {
    let url = harness.url();
    let options = PgConnectOptions::from_str(&url).unwrap();
    let upstream = format!("{}:{}", options.get_host(), options.get_port());
    let proxy = Proxy::start(upstream.clone()).await;
    let database = Database::connect(&proxy.url(&url, &upstream), 2)
        .await
        .unwrap();
    (proxy, database)
}

#[tokio::test]
async fn a_read_a_failed_connection_interrupted_is_read_again() {
    let harness = Harness::new().await;
    let repository = repository(&harness).await;
    let id = harness
        .log()
        .append(repository, changeset(1))
        .await
        .unwrap();
    harness
        .log()
        .compare_and_swap(repository, Ref::Main, None, id)
        .await
        .unwrap();
    let (proxy, database) = through_a_proxy(&harness).await;
    let log = PostgresChangesetLog::new(&database);

    proxy.arm(Fault::Request(b"select target from refs"));
    let main = log.read_ref(repository, Ref::Main).await;

    assert_eq!(main, Ok(Some(id)));
    assert!(proxy.spent(), "the connection was failed");
}

#[tokio::test]
async fn a_write_that_committed_before_the_connection_failed_is_written_again_without_harm() {
    let harness = Harness::new().await;
    let repository = repository(&harness).await;
    let (proxy, database) = through_a_proxy(&harness).await;
    let log = PostgresChangesetLog::new(&database);

    proxy.arm(Fault::Reply(b"INSERT 0 1\0"));
    let id = log.append(repository, changeset(1)).await.unwrap();

    assert!(proxy.spent(), "the connection was failed");
    assert_eq!(
        harness.log().get(repository, id).await.unwrap(),
        changeset(1)
    );
}

/// A runner registered for all of `acme`, which has no job and no run.
#[cfg(test)]
async fn idle_runner(harness: &Harness) -> Registration {
    repository(harness).await;
    let registered = NewRegistration {
        organisation: "acme".parse().unwrap(),
        repository: None,
        principal: Harness::principal("bot"),
        labels: BTreeSet::<Label>::new(),
    };
    let id = harness.registry.register(registered.clone()).await.unwrap();
    Registration { id, registered }
}

#[tokio::test]
async fn a_claim_with_nothing_waiting_opens_no_transaction_and_locks_no_row() {
    let harness = Harness::new().await;
    let runner = idle_runner(&harness).await;
    let (proxy, database) = through_a_proxy(&harness).await;
    let queue = PostgresJobQueue::new(&database);

    // The transaction of a job claim begins with BEGIN, and the statement that takes a job or a
    // run with `with next as`: neither reaches the server.
    proxy.arm(Fault::Request(b"BEGIN"));
    assert!(
        queue
            .claim(std::slice::from_ref(&runner), 0, 30)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!proxy.spent(), "a job claim began a transaction");
    proxy.arm(Fault::Request(b"with next as"));
    assert!(queue.claim_run(&[runner], 0, 30).await.unwrap().is_none());
    assert!(!proxy.spent(), "a run claim took a row to lock");
}
