//! The federated mode through the built `codehub` and `ch`: tokens from Keycloak presented from a
//! file or exchanged, identity by introspection, and in external mode the decisions of Cerbos
//! (policy in `dev/cerbos/policies`). There alice may do everything in `acme` and create
//! repositories, but not organisations, in `beta`; bob may read `acme/app` and write under `docs/`
//! in it.

#[cfg(test)]
pub mod common;

use common::{Server, TestDatabase, TokenUser, env};

/// The subject of Keycloak's user `bob`, as the policy names him.
#[cfg(test)]
fn bob_subject() -> String {
    env("CODEHUB_TEST_KEYCLOAK_SUBJECT_BOB")
}

#[tokio::test]
async fn ch_works_from_a_token_file_in_introspection_mode() {
    let database = TestDatabase::new().await;
    let server = Server::start_introspection(&database);
    let alice = TokenUser::new(&server, "TEST", "alice").await;

    alice.ok(&["org", "create", "acme"]).await;
    alice.ok(&["repo", "create", "acme/app"]).await;
    alice.ok(&["use", "acme/app"]).await;
    alice.stage("work", "docs/a.txt", "a\n").await;
    alice.ok(&["land", "-m", "a", "--draft", "work"]).await;
    assert_eq!(alice.ok(&["read", "docs/a.txt"]).await, "a\n");

    // The token stays out of the state file.
    let state = std::fs::read_to_string(alice.state_file()).unwrap();
    assert!(!state.contains(&alice.token()), "{state}");

    // The control: without a token file `ch` has no token, which the file supplied above.
    let nobody = common::User::new("alice");
    let missing = nobody
        .ch(&["--server", &server.base, "org", "create", "other"])
        .await;
    assert_eq!(missing.code, Some(3), "{}", missing.stderr);
}

#[tokio::test]
async fn ch_exchanges_a_token_by_rfc_8693_for_one_the_server_accepts() {
    let database = TestDatabase::new().await;
    let server = Server::start_oidc(&database);
    let alice = TokenUser::new(&server, "UPSTREAM", "alice").await;

    // The upstream client's token is not for codehub's audience.
    let direct = alice.ch(&["org", "create", "acme"]).await;
    assert_eq!(direct.code, Some(3), "{}", direct.stderr);

    // The same token, exchanged by `ch`, is.
    let exchanged = alice.ch_exchanging(&["org", "create", "acme"]).await;
    assert_eq!(exchanged.code, Some(0), "{}", exchanged.stderr);
    alice.ch_exchanging(&["repo", "create", "acme/app"]).await;
}

#[tokio::test]
async fn ch_with_a_token_for_another_audience_is_rejected_with_exit_3() {
    let database = TestDatabase::new().await;
    let server = Server::start_introspection(&database);
    let other = TokenUser::new(&server, "OTHER", "alice").await;

    let rejected = other.ch(&["org", "create", "acme"]).await;
    assert_eq!(rejected.code, Some(3), "{}", rejected.stderr);

    // The control: alice's token for codehub's audience is accepted by the same server.
    let alice = TokenUser::new(&server, "TEST", "alice").await;
    alice.ok(&["org", "create", "acme"]).await;
}

#[tokio::test]
async fn ch_with_a_revoked_token_is_rejected_at_once_in_introspection_mode_with_exit_3() {
    let database = TestDatabase::new().await;
    let server = Server::start_introspection(&database);
    let alice = TokenUser::new(&server, "TEST", "alice").await;
    alice.ok(&["org", "create", "acme"]).await;

    let revoked = reqwest::Client::new()
        .post(format!(
            "{}/protocol/openid-connect/revoke",
            env("CODEHUB_TEST_KEYCLOAK_ISSUER")
        ))
        .basic_auth(
            env("CODEHUB_TEST_KEYCLOAK_TEST_CLIENT_ID"),
            Some(env("CODEHUB_TEST_KEYCLOAK_TEST_CLIENT_SECRET")),
        )
        .form(&[("token", alice.token())])
        .send()
        .await
        .unwrap();
    assert!(revoked.status().is_success());

    // The same token file, the very next command.
    let rejected = alice.ch(&["repo", "create", "acme/app"]).await;
    assert_eq!(rejected.code, Some(3), "{}", rejected.stderr);
}

#[tokio::test]
async fn applies_the_decision_points_allow_to_land_and_deny_naming_the_denied_path() {
    let database = TestDatabase::new().await;
    let server = Server::start_external(&database);
    let alice = TokenUser::new(&server, "TEST", "alice").await;
    let bob = TokenUser::new(&server, "TEST", "bob").await;
    alice.ok(&["repo", "create", "acme/app"]).await;
    alice.ok(&["use", "acme/app"]).await;
    bob.ok(&["use", "acme/app"]).await;

    // bob's `put` of content is refused, because the upload is decided on the repository and the
    // policy gives bob write only where a path is named, so the changes under test are
    // deletions, which name their paths.
    alice.stage("seed", "docs/a.txt", "a\n").await;
    alice.stage("seed", "docs/b.txt", "b\n").await;
    alice.stage("seed", "src/c.txt", "c\n").await;
    alice.ok(&["land", "-m", "seed", "--draft", "seed"]).await;

    // Allowed: bob lands a change under docs/.
    bob.ok(&["put", "docs/a.txt", "--delete", "--draft", "docs"])
        .await;
    bob.ok(&["land", "-m", "docs", "--draft", "docs"]).await;
    assert_ne!(alice.ch(&["read", "docs/a.txt"]).await.code, Some(0));

    // Denied: bob may not change src/, and the refusal names that path and not docs/. It comes at
    // staging, which asks the decision point about each path before a landing can hold it.
    bob.ok(&["put", "docs/b.txt", "--delete", "--draft", "mixed"])
        .await;
    let denied = bob
        .ch(&["put", "src/c.txt", "--delete", "--draft", "mixed"])
        .await;
    assert_eq!(denied.code, Some(5), "{}", denied.stderr);
    assert!(denied.stderr.lines().any(|line| line == "src/c.txt"));
    assert!(!denied.stderr.contains("docs/b.txt"), "{}", denied.stderr);
    // Nothing of it landed, and the part under docs/ lands.
    assert_eq!(alice.ok(&["read", "src/c.txt"]).await, "c\n");
    bob.ok(&["land", "-m", "mixed", "--draft", "mixed"]).await;
    assert_ne!(alice.ch(&["read", "docs/b.txt"]).await.code, Some(0));
    assert_eq!(alice.ok(&["read", "src/c.txt"]).await, "c\n");
}

#[tokio::test]
async fn refuses_member_add_and_remove_in_external_mode_saying_members_are_administered_outside() {
    let database = TestDatabase::new().await;
    let server = Server::start_external(&database);
    let alice = TokenUser::new(&server, "TEST", "alice").await;
    alice.ok(&["repo", "create", "acme/app"]).await;
    alice.ok(&["use", "acme/app"]).await;
    let issuer = env("CODEHUB_TEST_KEYCLOAK_ISSUER");
    let bob = bob_subject();

    let add = alice
        .ch(&[
            "member", "add", &bob, "--issuer", &issuer, "--write", "docs",
        ])
        .await;
    let remove = alice
        .ch(&[
            "member", "remove", &bob, "--issuer", &issuer, "--write", "docs",
        ])
        .await;

    for run in [add, remove] {
        assert_eq!(run.code, Some(5), "{}", run.stderr);
        assert!(
            run.stderr
                .contains("members are administered outside codehub"),
            "{}",
            run.stderr
        );
    }
    // The control: alice, who may do everything in acme, lands a file, so the refusal is the
    // member endpoints' and not a denial of alice.
    alice.stage("work", "a.txt", "a\n").await;
    alice.ok(&["land", "-m", "a", "--draft", "work"]).await;
}

#[tokio::test]
async fn creates_an_organisation_on_first_use_when_the_decision_point_allows_it() {
    let database = TestDatabase::new().await;
    let server = Server::start_external(&database);
    let alice = TokenUser::new(&server, "TEST", "alice").await;

    // No `ch org create`: the repository's creation creates `acme`.
    alice.ok(&["repo", "create", "acme/app"]).await;

    alice.ok(&["use", "acme/app"]).await;
    alice.stage("work", "a.txt", "a\n").await;
    alice.ok(&["land", "-m", "a", "--draft", "work"]).await;
    assert_eq!(alice.ok(&["read", "a.txt"]).await, "a\n");
}

#[tokio::test]
async fn does_not_create_an_organisation_when_the_decision_point_denies_it() {
    let database = TestDatabase::new().await;
    let server = Server::start_external(&database);
    let alice = TokenUser::new(&server, "TEST", "alice").await;

    // alice may create repositories in beta, and not the organisation.
    let denied = alice.ch(&["repo", "create", "beta/app"]).await;
    assert_ne!(denied.code, Some(0));
    assert!(
        denied.stderr.contains("no organisation beta"),
        "{}",
        denied.stderr
    );

    // beta is absent: a following request is answered with the same missing organisation.
    let again = alice.ch(&["repo", "create", "beta/app"]).await;
    assert!(
        again.stderr.contains("no organisation beta"),
        "{}",
        again.stderr
    );

    // The control: where `CreateOrganisation` is allowed, the same call creates it.
    alice.ok(&["repo", "create", "acme/app"]).await;
}
