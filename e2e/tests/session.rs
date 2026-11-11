//! Logging in, setting up an organisation with a member, and the two ways a request is refused
//! before any landing: no token, and not a member. `codehub` and `ch` are the built binaries, run
//! against PostgreSQL, MinIO and Dex.

#[cfg(test)]
pub mod common;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{Server, TestDatabase, User, env, unique};
use serde_json::Value;

/// The claims of the JWT `token`.
#[cfg(test)]
fn claims(token: &str) -> Value {
    let payload = token.split('.').nth(1).unwrap();
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap()
}

#[tokio::test]
async fn logs_in_by_the_device_grant_with_a_token_for_ch_that_the_server_accepts() {
    let database = TestDatabase::new().await;
    let server = Server::start(&database);
    let alice = User::new("alice");

    let principal = alice.login(&server).await;

    assert_eq!(principal.issuer, env("CODEHUB_TEST_OIDC_ISSUER"));
    assert!(!principal.subject.is_empty());
    let state: Value =
        serde_json::from_str(&std::fs::read_to_string(alice.state_file()).unwrap()).unwrap();
    assert_eq!(state["server"], server.base);
    let aud = claims(state["token"].as_str().unwrap())["aud"].clone();
    assert!(
        aud == "ch"
            || aud
                .as_array()
                .is_some_and(|all| all.iter().any(|one| one == "ch")),
        "aud is {aud}"
    );
    // The server, which accepts audience ch only, takes the token.
    assert_eq!(
        alice
            .ok(&["org", "create", &unique("org")])
            .await
            .lines()
            .count(),
        1
    );
}

#[tokio::test]
async fn creates_an_organisation_and_a_repository_and_adds_a_member() {
    let database = TestDatabase::new().await;
    let server = Server::start(&database);
    let (alice, bob) = (User::new("alice"), User::new("bob"));
    alice.login(&server).await;
    let bob_principal = bob.login(&server).await;
    let org = unique("org");

    assert_eq!(
        alice.ok(&["org", "create", &org]).await,
        format!("created organisation {org}\n")
    );
    assert_eq!(
        alice.ok(&["repo", "create", &format!("{org}/app")]).await,
        format!("created repository {org}/app\n")
    );
    alice.ok(&["use", &format!("{org}/app")]).await;
    bob.ok(&["use", &format!("{org}/app")]).await;

    // Before the member is added the repository is not bob's to see.
    let before = bob.put("work", "docs/a.txt", "a\n").await;
    assert_eq!(before.code, Some(5), "{}", before.stderr);

    alice
        .ok(&["member", "add", &bob_principal.subject, "--write", "docs"])
        .await;
    bob.stage("work", "docs/a.txt", "a\n").await;
    bob.ok(&["land", "-m", "bob lands", "--draft", "work"])
        .await;
    assert_eq!(bob.ok(&["read", "docs/a.txt"]).await, "a\n");
}

#[tokio::test]
async fn rejects_a_request_without_a_token_with_exit_3() {
    let database = TestDatabase::new().await;
    let server = Server::start(&database);
    let alice = User::new("alice");

    // Never logged in: `ch` has no token to send.
    let nobody = User::new("alice");
    let missing = nobody.ch(&["org", "create", &unique("org")]).await;
    assert_eq!(missing.code, Some(3), "{}", missing.stderr);
    assert!(missing.stderr.contains("ch login"), "{}", missing.stderr);

    // A request the server receives without a token is answered 401.
    let http = reqwest::Client::new();
    let bare = http
        .post(format!("{}/v1/orgs", server.base))
        .json(&serde_json::json!({"name": unique("org")}))
        .send()
        .await
        .unwrap();
    assert_eq!(bare.status(), 401);

    // The control: the same request with alice's token is accepted.
    alice.login(&server).await;
    alice.ok(&["org", "create", &unique("org")]).await;
}

#[tokio::test]
async fn rejects_a_request_from_a_non_member_with_exit_5() {
    let database = TestDatabase::new().await;
    let server = Server::start(&database);
    let (alice, carol) = (User::new("alice"), User::new("carol"));
    alice.login(&server).await;
    let carol_principal = carol.login(&server).await;
    let org = unique("org");
    alice.create_repository(&org, "app").await;
    alice.stage("work", "a.txt", "a\n").await;
    alice.ok(&["land", "-m", "a", "--draft", "work"]).await;
    carol.ok(&["use", &format!("{org}/app")]).await;

    let denied = carol.ch(&["read", "a.txt"]).await;
    assert_eq!(denied.code, Some(5), "{}", denied.stderr);
    assert!(denied.stdout.is_empty());

    // The control: once carol is a member, the same read succeeds.
    alice
        .ok(&["member", "add", &carol_principal.subject, "--read", ""])
        .await;
    assert_eq!(carol.ok(&["read", "a.txt"]).await, "a\n");
}
