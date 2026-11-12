//! `ch runner`, run as the built binary against the HTTP API served in process.

#[cfg(test)]
pub mod common;

use common::{Server, repository, serve};
use serde_json::Value;

#[tokio::test]
async fn registers_lists_and_removes_a_runner() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;

    let first = ada
        .ok(&[
            "runner",
            "register",
            "runner-one",
            "--issuer",
            Server::ISSUER,
            "--label",
            "linux",
            "--label",
            "gpu",
        ])
        .await;
    let second = ada
        .ok(&[
            "runner",
            "register",
            "runner-two",
            "--issuer",
            Server::ISSUER,
        ])
        .await;
    let (first, second) = (first.trim(), second.trim());
    assert!(first.parse::<u64>().is_ok(), "{first}");
    assert_ne!(first, second);

    let issuer = Server::ISSUER;
    let expected = format!(
        "{first} organisation {issuer} runner-one gpu,linux\n{second} organisation {issuer} runner-two -\n"
    );
    assert_eq!(ada.ok(&["runner", "list"]).await, expected);

    ada.ok(&["runner", "remove", first]).await;
    assert_eq!(
        ada.ok(&["runner", "list"]).await,
        format!("{second} organisation {issuer} runner-two -\n")
    );

    // The control: a registration that is gone is not found.
    let again = ada.ch(&["runner", "remove", first]).await;
    assert_eq!(again.code, Some(5), "{}", again.stderr);
}

#[tokio::test]
async fn registers_a_runner_for_one_repository() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;

    ada.ok(&[
        "runner",
        "register",
        "scoped",
        "--issuer",
        Server::ISSUER,
        "--repo",
        "app",
    ])
    .await;
    ada.ok(&["runner", "register", "wide", "--issuer", Server::ISSUER])
        .await;

    let listed: Value = serde_json::from_str(&ada.ok(&["runner", "list", "--json"]).await).unwrap();
    let scopes: Vec<(&str, bool)> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|runner| {
            (
                runner["subject"].as_str().unwrap(),
                runner["repository_id"].is_u64(),
            )
        })
        .collect();
    assert_eq!(scopes, [("scoped", true), ("wide", false)]);
    let text = ada.ok(&["runner", "list"]).await;
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].contains(" repository:"), "{text}");
    assert!(lines[1].contains(" organisation "), "{text}");

    let missing = ada
        .ch(&[
            "runner",
            "register",
            "lost",
            "--issuer",
            Server::ISSUER,
            "--repo",
            "nothing",
        ])
        .await;
    assert_eq!(missing.code, Some(5), "{}", missing.stderr);
}

#[tokio::test]
async fn refuses_a_non_admin_the_runner_verbs_with_exit_5() {
    let server = serve().await;
    let ada = server.user("ada");
    let bob = server.user("bob");
    let org = repository(&ada).await;
    // bob is a member with the default role, not an admin.
    ada.ok(&["member", "add", "bob", "--write", ""]).await;
    bob.ok(&["use", &format!("{org}/app")]).await;

    let args = ["runner", "register", "r1", "--issuer", Server::ISSUER];
    let refused = bob.ch(&args).await;
    assert_eq!(refused.code, Some(5), "{}", refused.stderr);
    assert!(refused.stdout.is_empty());
    let listing = bob.ch(&["runner", "list"]).await;
    assert_eq!(listing.code, Some(5), "{}", listing.stderr);

    // The control: the admin registers the same runner, so the refusal was the role.
    let id = ada.ok(&args).await;
    let removal = bob.ch(&["runner", "remove", id.trim()]).await;
    assert_eq!(removal.code, Some(5), "{}", removal.stderr);
    assert_eq!(ada.ok(&["runner", "list"]).await.lines().count(), 1);
}
