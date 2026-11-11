//! Data `codehub` holds outlives the process: stop it, start another over the same database and
//! object store, and read what was landed.

#[cfg(test)]
pub mod common;

use common::{Server, TestDatabase, User, unique};

#[tokio::test]
async fn keeps_landed_data_across_a_restart_of_codehub() {
    let database = TestDatabase::new().await;
    let mut first = Server::start(&database);
    let alice = User::new("alice");
    alice.login(&first).await;
    let org = unique("org");
    alice.create_repository(&org, "app").await;
    alice.stage("work", "docs/a.txt", "one\n").await;
    alice.ok(&["land", "-m", "one", "--draft", "work"]).await;
    alice.stage("work", "docs/a.txt", "two\n").await;
    alice.ok(&["land", "-m", "two", "--draft", "work"]).await;
    let log = alice.ok(&["log", "docs/a.txt"]).await;
    assert_eq!(log.lines().count(), 2);
    assert_eq!(alice.ok(&["read", "docs/a.txt"]).await, "two\n");

    assert!(first.terminate().success());
    let second = Server::start(&database);
    assert_ne!(first.base, second.base);

    // The new process answers on a new port, so alice logs in at it and selects the repository.
    alice.login(&second).await;
    alice.ok(&["use", &format!("{org}/app")]).await;
    assert_eq!(alice.ok(&["read", "docs/a.txt"]).await, "two\n");
    assert_eq!(alice.ok(&["log", "docs/a.txt"]).await, log);
}
