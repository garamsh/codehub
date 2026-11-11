//! Landings through `ch` against the built `codehub`: what lands, what is rejected and naming
//! which path, and what `log` and `read` return afterwards.

#[cfg(test)]
pub mod common;

use common::{Server, TestDatabase, User, unique};
use sha2::{Digest, Sha256};

/// The address `ch log` prints for `content`.
#[cfg(test)]
fn address(content: &str) -> String {
    let digest: String = Sha256::digest(content.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{digest}")
}

#[tokio::test]
async fn lands_two_users_changes_on_disjoint_paths() {
    let database = TestDatabase::new().await;
    let server = Server::start(&database);
    let (alice, bob) = (User::new("alice"), User::new("bob"));
    alice.login(&server).await;
    let bob_principal = bob.login(&server).await;
    let org = unique("org");
    alice.create_repository(&org, "app").await;
    alice
        .ok(&["member", "add", &bob_principal.subject, "--write", ""])
        .await;
    bob.ok(&["use", &format!("{org}/app")]).await;

    // Both drafts are staged before either lands.
    alice.stage("work", "alice/a.txt", "from alice\n").await;
    bob.stage("work", "bob/b.txt", "from bob\n").await;
    let first = alice.ok(&["land", "-m", "alice", "--draft", "work"]).await;
    let second = bob.ok(&["land", "-m", "bob", "--draft", "work"]).await;

    assert_ne!(first, second);
    assert_eq!(alice.ok(&["read", "bob/b.txt"]).await, "from bob\n");
    assert_eq!(bob.ok(&["read", "alice/a.txt"]).await, "from alice\n");
}

#[tokio::test]
async fn rejects_a_landing_changing_the_same_lines_differently_naming_the_path_and_exits_4() {
    let database = TestDatabase::new().await;
    let server = Server::start(&database);
    let alice = User::new("alice");
    alice.login(&server).await;
    alice.create_repository(&unique("org"), "app").await;
    alice.stage("base", "poem.txt", "one\ntwo\nthree\n").await;
    alice.ok(&["land", "-m", "base", "--draft", "base"]).await;

    // Three drafts from the same base: x and y change line 2 differently, z changes line 1.
    alice.stage("x", "poem.txt", "one\nTWO\nthree\n").await;
    alice.stage("y", "poem.txt", "one\n2\nthree\n").await;
    alice.stage("z", "poem.txt", "1\ntwo\nthree\n").await;
    alice.ok(&["land", "-m", "x", "--draft", "x"]).await;

    let conflict = alice.ch(&["land", "-m", "y", "--draft", "y"]).await;
    assert_eq!(conflict.code, Some(4), "{}", conflict.stderr);
    assert!(conflict.stderr.lines().any(|line| line == "poem.txt"));
    assert!(conflict.stdout.is_empty());

    // The control: the same path, other lines, lands over x.
    alice.ok(&["land", "-m", "z", "--draft", "z"]).await;
    assert_eq!(alice.ok(&["read", "poem.txt"]).await, "1\nTWO\nthree\n");
}

#[tokio::test]
async fn rejects_a_landing_changing_a_path_the_user_may_not_write_naming_the_path_and_exits_4() {
    let database = TestDatabase::new().await;
    let server = Server::start(&database);
    let (alice, bob) = (User::new("alice"), User::new("bob"));
    alice.login(&server).await;
    let bob_principal = bob.login(&server).await;
    let org = unique("org");
    alice.create_repository(&org, "app").await;
    let subject = bob_principal.subject.as_str();
    alice.ok(&["member", "add", subject, "--write", ""]).await;
    bob.ok(&["use", &format!("{org}/app")]).await;

    // bob stages under docs and src while he may write everywhere; then he may write src only.
    bob.stage("work", "docs/x.txt", "x\n").await;
    bob.stage("work", "src/y.txt", "y\n").await;
    bob.stage("safe", "src/z.txt", "z\n").await;
    alice
        .ok(&["member", "remove", subject, "--write", ""])
        .await;
    alice
        .ok(&["member", "add", subject, "--write", "src"])
        .await;

    let denied = bob.ch(&["land", "-m", "work", "--draft", "work"]).await;
    assert_eq!(denied.code, Some(4), "{}", denied.stderr);
    assert!(denied.stderr.lines().any(|line| line == "docs/x.txt"));
    assert!(!denied.stderr.contains("src/y.txt"));

    // The control: a draft inside what he may still write lands.
    bob.ok(&["land", "-m", "safe", "--draft", "safe"]).await;
}

#[tokio::test]
async fn returns_the_version_chain_from_log_and_the_content_from_read() {
    let database = TestDatabase::new().await;
    let server = Server::start(&database);
    let alice = User::new("alice");
    alice.login(&server).await;
    alice.create_repository(&unique("org"), "app").await;

    alice.stage("work", "src/a.txt", "one\n").await;
    let first = alice.ok(&["land", "-m", "first", "--draft", "work"]).await;
    alice.stage("work", "src/a.txt", "two\n").await;
    let second = alice.ok(&["land", "-m", "second", "--draft", "work"]).await;
    let (first, second) = (first.trim(), second.trim());
    assert_ne!(first, second);

    assert_eq!(alice.ok(&["read", "src/a.txt"]).await, "two\n");
    assert_eq!(
        alice.ok(&["log", "src/a.txt"]).await,
        format!(
            "{second} {}\n{first} {}\n",
            address("two\n"),
            address("one\n")
        )
    );
}
