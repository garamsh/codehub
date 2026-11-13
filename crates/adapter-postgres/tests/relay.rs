//! The directory outbox in PostgreSQL: a creation's entry is written in its transaction, and the
//! relay loses none across a restart.

#[cfg(test)]
pub mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use adapter_postgres::{Database, PostgresDirectoryOutbox};
use app::domain::name::{OrganisationName, RepositoryName};
use app::port::driven::directory_changes::{
    DirectoryChanges, OrganisationChange, RepositoryChange,
};
use app::port::driven::directory_outbox::DirectoryOutbox;
use app::port::driving::repositories::Repositories;
use app::use_case::relay::{DirectoryRelay, LEASE, Pass};
use common::Harness;
use sqlx::{Connection, PgConnection};
use test_support::contract::{Deployment, RealTime};

/// A receiver that refuses every repository change until it is switched on.
#[cfg(test)]
struct Switch(AtomicBool);

#[cfg(test)]
impl DirectoryChanges for Switch {
    async fn repository_changed(&self, _change: &RepositoryChange) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    async fn organisation_changed(&self, _change: &OrganisationChange) {}
}

/// A relay over a pool of its own to the harness's database, as a restarted server makes one.
#[cfg(test)]
async fn restarted(
    harness: &Harness,
    receiver: &Arc<Switch>,
) -> DirectoryRelay<PostgresDirectoryOutbox, Switch, RealTime> {
    let database = Database::connect(&harness.url(), 2).await.unwrap();
    DirectoryRelay::new(
        Arc::new(PostgresDirectoryOutbox::new(&database)),
        Arc::clone(receiver),
        Arc::new(RealTime),
        Duration::from_secs(2),
    )
}

#[cfg(test)]
async fn count(harness: &Harness, table: &str) -> i64 {
    let mut connection = PgConnection::connect(&harness.url()).await.unwrap();
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("select count(*) from {table}")))
        .fetch_one(&mut connection)
        .await
        .unwrap()
}

#[tokio::test]
async fn loses_no_entry_across_a_restart_while_the_receiver_is_down() {
    let harness = Harness::new().await;
    let ada = harness.caller("ada").await;
    let id = harness.repository(&ada, "acme").await.id;
    let receiver = Arc::new(Switch(AtomicBool::new(false)));
    let before = restarted(&harness, &receiver).await;
    assert_eq!(before.pass(None, LEASE).await, Pass::Failed);
    drop(before);

    // The server starts again over the same database; the receiver is up by then.
    receiver.0.store(true, Ordering::SeqCst);
    let after = restarted(&harness, &receiver).await;
    let mut delivered = false;
    for _ in 0..100 {
        if after.pass(None, LEASE).await == Pass::Delivered {
            delivered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(delivered, "the entry was not delivered after the restart");
    assert_eq!(count(&harness, "directory_outbox").await, 0);
    assert_eq!(harness.outbox.claim(Some(id), 0, 0).await.unwrap(), vec![]);
}

#[tokio::test]
async fn a_creation_whose_entry_cannot_be_written_creates_no_repository() {
    let harness = Harness::new().await;
    let ada = harness.caller("ada").await;
    let acme: OrganisationName = "acme".parse().unwrap();
    let app: RepositoryName = "app".parse().unwrap();
    harness
        .repositories()
        .create_organisation(&ada, &acme)
        .await
        .unwrap();
    let mut connection = PgConnection::connect(&harness.url()).await.unwrap();
    sqlx::query(
        "alter table directory_outbox add constraint refuses_entries check (false) not valid",
    )
    .execute(&mut connection)
    .await
    .unwrap();

    let refused = harness
        .repositories()
        .create_repository(&ada, &acme, &app)
        .await;

    assert!(refused.is_err());
    assert_eq!(count(&harness, "repositories").await, 0);
    assert_eq!(count(&harness, "directory_outbox").await, 0);
    // Control: without the failing write the same creation commits, with its entry.
    sqlx::query("alter table directory_outbox drop constraint refuses_entries")
        .execute(&mut connection)
        .await
        .unwrap();
    harness
        .repositories()
        .create_repository(&ada, &acme, &app)
        .await
        .unwrap();
    assert_eq!(count(&harness, "repositories").await, 1);
    assert_eq!(count(&harness, "directory_outbox").await, 1);
}
