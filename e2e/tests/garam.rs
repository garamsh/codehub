//! The built `codehub` and `ch` against garam's published images (`dev/garam`), with garam as the
//! OIDC issuer and the AuthZEN decision point, in the `grn` profile with `private_key_jwt`.
//!
//! These tests need the stack that `make e2e-garam` starts and stops, and `make e2e` skips them.
//! The tests that expect garam to allow something write the role links the platform writes, with
//! `CreatePolicy` as the platform. The directory outbox tells garam about each repository, and a
//! test that depends on garam knowing a repository waits, bounded, until garam holds it.
//!
//! In external mode `ch` exits 3 for a token the server rejects, 5 for a decision that denies (a
//! 403, and a 404 for what the caller may not see, such as a repository garam does not hold) and
//! 1 for a server error such as a decision point that could not be asked.

#[cfg(test)]
pub mod common;

use common::garam::Garam;
use common::{TestDatabase, TokenUser};

#[tokio::test]
async fn garam_token_authenticates_a_person_and_another_audience_does_not() {
    let garam = Garam::running();
    let person = garam.person().await;
    let database = TestDatabase::new().await;
    let server = garam.server(&database, &garam.key("codehub.pem"));

    // A token for the policy audience is not codehub's to accept.
    let other = TokenUser::with_token(&server, "other-audience", &person.other_audience_token);
    let rejected = other.ch(&["org", "create", &person.org]).await;
    assert_eq!(rejected.code, Some(3), "{}", rejected.stderr);

    // The control: the person's token for codehub is authenticated, so the request goes on to
    // garam's decision, which denies it for want of a role link.
    let owner = TokenUser::with_token(&server, "owner", &person.token);
    let denied = owner.ch(&["org", "create", &person.org]).await;
    assert_eq!(denied.code, Some(5), "{}", denied.stderr);
}

#[tokio::test]
async fn garam_denies_creating_an_organisation_and_a_repository_until_the_owner_has_a_role_link() {
    let garam = Garam::running();
    let person = garam.person().await;
    let database = TestDatabase::new().await;
    let server = garam.server(&database, &garam.key("codehub.pem"));
    let owner = TokenUser::with_token(&server, "owner", &person.token);
    let repository = format!("{}/app", person.org);

    // The person owns the organisation in garam's directory and holds no role link in its
    // policy service: garam's decision denies, which `ch` reports as denied, not as an error.
    let organisation = owner.ch(&["org", "create", &person.org]).await;
    assert_eq!(organisation.code, Some(5), "{}", organisation.stderr);
    let denied = owner.ch(&["repo", "create", &repository]).await;
    assert_eq!(denied.code, Some(5), "{}", denied.stderr);

    // The control: a server that cannot ask garam's decision point answers a server error, so
    // the exit 5 above is not one.
    let unreachable = garam.server_without_decision_point(&database);
    let blind = TokenUser::with_token(&unreachable, "blind", &person.token);
    let error = blind.ch(&["org", "create", &person.org]).await;
    assert_eq!(error.code, Some(1), "{}", error.stderr);

    // The control: with the role link the same requests are accepted, so the denials above are
    // the missing role link's and not garam's denying everything.
    garam.make_owner(&person);
    owner.ok(&["org", "create", &person.org]).await;
    owner.ok(&["repo", "create", &repository]).await;
}

#[tokio::test]
async fn garam_refuses_a_key_it_does_not_hold_and_the_request_allows_nothing() {
    let garam = Garam::running();
    let person = garam.person().await;
    let database = TestDatabase::new().await;

    // `other.pem` is signed with, but garam holds the public half of `codehub.pem` alone: the
    // token request is refused, which is a server error that allows nothing.
    let wrong = garam.server(&database, &garam.key("other.pem"));
    let owner = TokenUser::with_token(&wrong, "owner", &person.token);
    let refused = owner.ch(&["org", "create", &person.org]).await;
    assert_eq!(refused.code, Some(1), "{}", refused.stderr);

    // The control: the key garam holds gets a token from garam, with which codehub reaches
    // garam's decision.
    let right = garam.server(&database, &garam.key("codehub.pem"));
    let owner = TokenUser::with_token(&right, "owner", &person.token);
    let denied = owner.ch(&["org", "create", &person.org]).await;
    assert_eq!(denied.code, Some(5), "{}", denied.stderr);
}

#[tokio::test]
async fn garam_allows_an_owner_to_create_a_repository() {
    let garam = Garam::running();
    let person = garam.person().await;
    garam.make_owner(&person);
    let database = TestDatabase::new().await;
    let server = garam.server(&database, &garam.key("codehub.pem"));
    let owner = TokenUser::with_token(&server, "owner", &person.token);

    owner.ok(&["org", "create", &person.org]).await;
    owner
        .ok(&["repo", "create", &format!("{}/app", person.org)])
        .await;

    // The control: another person's token is denied the same repository.
    let stranger = garam.person().await;
    let stranger = TokenUser::with_token(&server, "stranger", &stranger.token);
    let denied = stranger
        .ch(&["repo", "create", &format!("{}/other", person.org)])
        .await;
    assert_eq!(denied.code, Some(5), "{}", denied.stderr);
}

#[tokio::test]
async fn garam_holds_a_repository_once_the_outbox_delivers_it() {
    let garam = Garam::running();
    let delivering = garam.person().await;
    let discarding = garam.person().await;
    garam.make_owner(&delivering);
    garam.make_owner(&discarding);
    let database = TestDatabase::new().await;
    let key = garam.key("codehub.pem");
    let server = garam.server(&database, &key);
    let owner = TokenUser::with_token(&server, "owner", &delivering.token);
    let other_database = TestDatabase::new().await;
    let silent = garam.server_without_delivery(&other_database, &key);
    let silent_owner = TokenUser::with_token(&silent, "silent", &discarding.token);

    // The control: where the outbox discards its entries, the repository is created and never
    // reaches garam. It is made first, so it has had longer than the delivered one below.
    silent_owner.ok(&["org", "create", &discarding.org]).await;
    let silent_id = {
        let out = silent_owner
            .ok(&[
                "--json",
                "repo",
                "create",
                &format!("{}/app", discarding.org),
            ])
            .await;
        serde_json::from_str::<serde_json::Value>(&out).unwrap()["id"]
            .as_u64()
            .unwrap()
    };

    owner.ok(&["org", "create", &delivering.org]).await;
    let id = garam
        .create_repository(&owner, &delivering.org, "app")
        .await;
    assert!(garam.holds_repository(&delivering.org, id));
    assert!(!garam.holds_repository(&discarding.org, silent_id));
}

#[tokio::test]
async fn garam_allows_an_owner_to_land_a_change() {
    let garam = Garam::running();
    let person = garam.person().await;
    garam.make_owner(&person);
    let database = TestDatabase::new().await;
    let server = garam.server(&database, &garam.key("codehub.pem"));
    let owner = TokenUser::with_token(&server, "owner", &person.token);
    let repository = format!("{}/app", person.org);
    owner.ok(&["org", "create", &person.org]).await;
    garam.create_repository(&owner, &person.org, "app").await;
    owner.ok(&["use", &repository]).await;

    owner.stage("work", "docs/a.txt", "a\n").await;
    owner.ok(&["land", "-m", "a", "--draft", "work"]).await;
    assert_eq!(owner.ok(&["read", "docs/a.txt"]).await, "a\n");
}

#[tokio::test]
async fn garam_denies_a_path_its_guardrail_forbids_beside_one_it_allows() {
    let garam = Garam::running();
    let person = garam.person().await;
    garam.make_owner(&person);
    garam.forbid_writes_under_src(&person);
    let database = TestDatabase::new().await;
    let server = garam.server(&database, &garam.key("codehub.pem"));
    let owner = TokenUser::with_token(&server, "owner", &person.token);
    let repository = format!("{}/app", person.org);
    owner.ok(&["org", "create", &person.org]).await;
    garam.create_repository(&owner, &person.org, "app").await;
    owner.ok(&["use", &repository]).await;

    // The control: a path outside the guardrail is staged and lands.
    owner.stage("work", "docs/a.txt", "a\n").await;
    owner.ok(&["land", "-m", "docs", "--draft", "work"]).await;

    let forbidden = owner.put("other", "src/a.txt", "a\n").await;
    assert_eq!(forbidden.code, Some(5), "{}", forbidden.stderr);
}
