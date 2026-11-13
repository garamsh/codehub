//! The verbs of `ch`, run as the built binary against the HTTP API served in process.

#[cfg(test)]
pub mod common;

use std::os::unix::fs::PermissionsExt;

use app::domain::address::Address;
use common::{User, file, put, repository, serve, unique};
use serde_json::{Value, json};

#[cfg(test)]
fn address(content: &str) -> String {
    Address::of(content.as_bytes()).to_string()
}

#[tokio::test]
async fn stages_lands_reads_and_logs_a_file() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;

    put(&ada, "default", "src/a.txt", "one\n").await;
    let first = ada.ok(&["land", "-m", "first"]).await.trim().to_owned();
    put(&ada, "default", "src/a.txt", "two\n").await;
    let second = ada
        .ok(&["land", "--message", "second"])
        .await
        .trim()
        .to_owned();
    assert_ne!(first, second);
    assert!(first.parse::<Address>().is_ok(), "{first}");

    assert_eq!(ada.ok(&["read", "src/a.txt"]).await, "two\n");
    let log = ada.ok(&["log", "src/a.txt"]).await;
    let expected = format!(
        "{second} {}\n{first} {}\n",
        address("two\n"),
        address("one\n")
    );
    assert_eq!(log, expected);
}

#[tokio::test]
async fn stages_a_deletion_and_reads_nothing_after_it_lands() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    put(&ada, "default", "a.txt", "one\n").await;
    ada.ok(&["land", "-m", "add"]).await;
    assert_eq!(ada.ok(&["read", "a.txt"]).await, "one\n");

    ada.ok(&["put", "--delete", "a.txt"]).await;
    ada.ok(&["land", "-m", "delete"]).await;
    let gone = ada.ch(&["read", "a.txt"]).await;
    assert_eq!(gone.code, Some(5), "{}", gone.stderr);
}

#[tokio::test]
async fn names_the_path_of_a_conflicting_landing_and_exits_4() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    put(&ada, "base", "a.txt", "one\n").await;
    ada.ok(&["land", "-m", "base", "--draft", "base"]).await;

    // Both drafts start from the same head; one edits a.txt, the other a.txt and b.txt.
    put(&ada, "x", "a.txt", "two\n").await;
    put(&ada, "y", "a.txt", "three\n").await;
    put(&ada, "y", "b.txt", "other\n").await;
    ada.ok(&["land", "-m", "x", "--draft", "x"]).await;

    let conflict = ada.ch(&["land", "-m", "y", "--draft", "y"]).await;
    assert_eq!(conflict.code, Some(4), "{}", conflict.stderr);
    assert!(conflict.stderr.lines().any(|line| line == "a.txt"));
    assert!(!conflict.stderr.contains("b.txt"));
    assert!(conflict.stdout.is_empty());

    // The control: a draft on a path the head did not change lands.
    put(&ada, "z", "c.txt", "fresh\n").await;
    ada.ok(&["land", "-m", "z", "--draft", "z"]).await;
}

#[tokio::test]
async fn names_the_path_of_a_landing_the_caller_may_no_longer_write_and_exits_4() {
    let server = serve().await;
    let ada = server.user("ada");
    let bob = server.user("bob");
    let org = repository(&ada).await;
    ada.ok(&["member", "add", "bob", "--write", ""]).await;
    bob.ok(&["use", &format!("{org}/app")]).await;

    // bob stages under docs and src while he may write everywhere, then may write src only.
    put(&bob, "work", "docs/x.txt", "x\n").await;
    put(&bob, "work", "src/y.txt", "y\n").await;
    put(&bob, "safe", "src/z.txt", "z\n").await;
    ada.ok(&["member", "remove", "bob", "--write", ""]).await;
    ada.ok(&["member", "add", "bob", "--write", "src"]).await;

    let denied = bob.ch(&["land", "-m", "work", "--draft", "work"]).await;
    assert_eq!(denied.code, Some(4), "{}", denied.stderr);
    assert!(denied.stderr.lines().any(|line| line == "docs/x.txt"));
    assert!(!denied.stderr.contains("src/y.txt"));

    // The control: a draft inside what he may still write lands.
    bob.ok(&["land", "-m", "safe", "--draft", "safe"]).await;
}

#[tokio::test]
async fn lets_a_member_with_a_write_grant_land_until_the_member_is_removed() {
    let server = serve().await;
    let ada = server.user("ada");
    let bob = server.user("bob");
    let org = repository(&ada).await;
    bob.ok(&["use", &format!("{org}/app")]).await;

    // Before `member add`, the repository is not bob's to see.
    let before = bob
        .ch(&["put", "src/x.txt", &file(b"x\n"), "--draft", "d"])
        .await;
    assert_eq!(before.code, Some(5), "{}", before.stderr);

    // The issuer defaults to the one in ada's token, which is bob's too.
    ada.ok(&["member", "add", "bob", "--write", "src"]).await;
    put(&bob, "d", "src/x.txt", "x\n").await;
    bob.ok(&["land", "-m", "bob lands", "--draft", "d"]).await;
    assert_eq!(bob.ok(&["read", "src/x.txt"]).await, "x\n");

    // A path outside the grant is refused, naming it.
    let outside = bob
        .ch(&["put", "docs/y.txt", &file(b"y\n"), "--draft", "e"])
        .await;
    assert_eq!(outside.code, Some(5), "{}", outside.stderr);
    assert!(outside.stderr.lines().any(|line| line == "docs/y.txt"));

    ada.ok(&["member", "remove", "bob"]).await;
    let after = bob.ch(&["read", "src/x.txt"]).await;
    assert_eq!(after.code, Some(5), "{}", after.stderr);
}

#[tokio::test]
async fn prints_json_for_land_read_log_and_create() {
    let server = serve().await;
    let ada = server.user("ada");
    let org = unique("org");
    let created: Value =
        serde_json::from_str(&ada.ok(&["--json", "org", "create", &org]).await).unwrap();
    assert_eq!(created, json!({"name": org}));
    ada.ok(&["repo", "create", &format!("{org}/app"), "--json"])
        .await;
    ada.ok(&["use", &format!("{org}/app")]).await;

    ada.ok(&["put", "tool.sh", &file(b"#!/bin/sh\n"), "--executable"])
        .await;
    let landed: Value =
        serde_json::from_str(&ada.ok(&["land", "-m", "tool", "--json"]).await).unwrap();
    let changeset = landed["changeset"].as_str().unwrap().to_owned();
    assert!(changeset.parse::<Address>().is_ok(), "{changeset}");

    let read: Value = serde_json::from_str(&ada.ok(&["read", "tool.sh", "--json"]).await).unwrap();
    assert_eq!(
        read,
        json!({"path": "tool.sh", "kind": "executable", "content": "#!/bin/sh\n"})
    );

    let log: Value = serde_json::from_str(&ada.ok(&["log", "tool.sh", "--json"]).await).unwrap();
    assert_eq!(
        log,
        json!([{
            "changeset": changeset,
            "path": "tool.sh",
            "kind": "executable",
            "content": address("#!/bin/sh\n"),
            "previous": [],
            "message": "tool",
        }])
    );
}

#[tokio::test]
async fn reads_bytes_that_are_not_utf8_as_base64_in_json_and_raw_otherwise() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    let bytes = [0xff, 0xfe, 0x00];
    ada.ok(&["put", "blob.bin", &file(&bytes)]).await;
    ada.ok(&["land", "-m", "blob"]).await;

    let read: Value = serde_json::from_str(&ada.ok(&["read", "blob.bin", "--json"]).await).unwrap();
    assert_eq!(
        read,
        json!({"path": "blob.bin", "kind": "regular", "content_base64": "//4A"})
    );
    let raw = ada.ch(&["read", "blob.bin"]).await;
    assert_eq!(raw.stdout, bytes);
}

#[tokio::test]
async fn exits_3_without_a_token_and_when_the_server_refuses_it() {
    let server = serve().await;

    let nobody = User::logged_out();
    let missing = nobody.ch(&["org", "create", &unique("org")]).await;
    assert_eq!(missing.code, Some(3), "{}", missing.stderr);
    assert!(missing.stderr.contains("ch login"));

    let stale = server.user_with_token("not-a-credential");
    let refused = stale.ch(&["org", "create", &unique("org")]).await;
    assert_eq!(refused.code, Some(3), "{}", refused.stderr);

    // The control: a credential the server knows makes the same request succeed.
    let ada = server.user("ada");
    ada.ok(&["org", "create", &unique("org")]).await;
}

#[tokio::test]
async fn keeps_the_state_private_and_to_three_items() {
    let server = serve().await;
    let ada = server.user("ada");
    ada.ok(&["use", "acme/app"]).await;

    let file = ada.state_file();
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&file), 0o600);
    let state: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let token = state["token"].as_str().unwrap();
    assert_eq!(ch::login::principal(token).unwrap().subject, "ada");
    assert_eq!(
        state,
        json!({"server": server.url(), "token": token, "repository": "acme/app"})
    );
}

#[tokio::test]
async fn asks_for_a_selected_repository_before_acting_on_one() {
    let server = serve().await;
    let ada = server.user("ada");
    let run = ada.ch(&["put", "a.txt", &file(b"x\n")]).await;
    assert_eq!(run.code, Some(1), "{}", run.stderr);
    assert!(run.stderr.contains("ch use"));
}
