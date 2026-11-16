//! The PostgreSQL authorisation decision over many resources at once: the same decisions as one
//! call each, from one read of what the principal holds.

#[cfg(test)]
pub mod common;

use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::path::{Path, Prefix};
use app::domain::principal::Principal;
use app::domain::repository::RepositoryId;
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, Decision, Resource,
};
use app::port::driven::membership::{Access, Grant, Membership, Role};
use app::port::driven::repository_directory::RepositoryDirectory;
use common::Harness;
use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection};

/// The organisation of every test here.
#[cfg(test)]
fn acme() -> OrganisationName {
    "acme".parse().unwrap()
}

/// The repository of every test here.
#[cfg(test)]
fn app() -> RepositoryName {
    "app".parse().unwrap()
}

/// The principal `subject` of one issuer.
#[cfg(test)]
fn principal(subject: &str) -> Principal {
    Principal::new("https://id.test", subject).unwrap()
}

/// `ada`, an admin; `bob`, a member with write on `src`, read on `docs` and run on the whole
/// repository; and `eve`, no member, in a repository the harness created.
#[cfg(test)]
async fn setup(harness: &Harness) -> RepositoryId {
    harness
        .directory
        .create_organisation(&acme())
        .await
        .unwrap();
    let repository = harness
        .directory
        .create_repository(&acme(), &app())
        .await
        .unwrap();
    harness
        .membership
        .grant_role(&acme(), &principal("ada"), Role::Admin)
        .await
        .unwrap();
    harness
        .membership
        .grant_role(&acme(), &principal("bob"), Role::Member)
        .await
        .unwrap();
    for (prefix, access) in [
        (Prefix::from("src".parse::<Path>().unwrap()), Access::Write),
        (Prefix::from("docs".parse::<Path>().unwrap()), Access::Read),
        (Prefix::whole(), Access::Run),
    ] {
        harness
            .membership
            .add_grant(
                &acme(),
                &principal("bob"),
                Grant {
                    repository,
                    prefix,
                    access,
                },
            )
            .await
            .unwrap();
    }
    repository
}

/// The path `path` of `repository`.
#[cfg(test)]
fn path(repository: RepositoryId, path: &str) -> Resource {
    Resource::Path {
        organisation: acme(),
        name: app(),
        repository,
        path: path.parse().unwrap(),
    }
}

/// The paths decided: inside `src` and `docs`, beside them by a shared leading name only, and
/// outside both.
#[cfg(test)]
fn paths(repository: RepositoryId) -> Vec<Resource> {
    [
        "src/a", "src/a/b", "srcx/a", "docs/x", "docs", "other/z", "src/a",
    ]
    .iter()
    .map(|name| path(repository, name))
    .collect()
}

/// The repository as a whole, which is what `run` is asked of.
#[cfg(test)]
fn whole(repository: RepositoryId) -> Resource {
    Resource::Repository {
        organisation: acme(),
        name: app(),
        repository,
    }
}

#[tokio::test]
async fn deciding_many_paths_answers_each_as_a_single_call_does() {
    let harness = Harness::new().await;
    let repository = setup(&harness).await;
    let mut resources = paths(repository);
    resources.push(whole(repository));
    let (allow, deny) = (Decision::Allow, Decision::Deny);

    for (subject, action, expected) in [
        // Prefixes match by whole components: `srcx/a` is outside `src`, and `docs` is read only.
        (
            "bob",
            Action::Write,
            [allow, allow, deny, deny, deny, deny, allow, allow],
        ),
        (
            "bob",
            Action::Read,
            [allow, allow, deny, allow, allow, deny, allow, allow],
        ),
        // A run is asked of the repository, so it is denied on every path, even where granted.
        (
            "bob",
            Action::Run,
            [deny, deny, deny, deny, deny, deny, deny, allow],
        ),
        (
            "ada",
            Action::Write,
            [allow, allow, allow, allow, allow, allow, allow, allow],
        ),
        (
            "ada",
            Action::Run,
            [deny, deny, deny, deny, deny, deny, deny, allow],
        ),
        (
            "eve",
            Action::Read,
            [deny, deny, deny, deny, deny, deny, deny, deny],
        ),
    ] {
        let subject = principal(subject);
        let batch = harness
            .authorisation
            .decide_all(&subject, action, &resources)
            .await
            .unwrap();
        let mut singles = Vec::new();
        for resource in &resources {
            singles.push(
                harness
                    .authorisation
                    .decide(&subject, action, resource)
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(batch, singles, "{subject} {action:?}");
        assert_eq!(batch, expected, "{subject} {action:?}");
    }
}

/// Runs `statement` on `connection`.
#[cfg(test)]
async fn run(connection: &mut PgConnection, statement: &str) {
    connection
        .execute(AssertSqlSafe(statement.to_owned()))
        .await
        .unwrap();
}

/// How many reads of a member the view of [`deciding_many_paths_reads_the_member_once`] counted.
#[cfg(test)]
async fn reads(connection: &mut PgConnection) -> i64 {
    sqlx::query_scalar("select case when is_called then last_value else 0 end from member_reads")
        .fetch_one(connection)
        .await
        .unwrap()
}

#[tokio::test]
async fn deciding_many_paths_reads_the_member_once() {
    let harness = Harness::new().await;
    let repository = setup(&harness).await;
    let resources = paths(repository);
    let subject = principal("bob");
    // Every read of a member passes through a view that counts it, so the decisions are counted
    // by what they read and not by what they are called.
    let mut connection = PgConnection::connect(&harness.url()).await.unwrap();
    for statement in [
        "create sequence member_reads",
        "alter table members rename to members_held",
        "create view members as select * from members_held where nextval('member_reads') > 0",
    ] {
        run(&mut connection, statement).await;
    }

    // Control: a single decision reads the same number of times whichever the resource, and that
    // number is not zero, so the counter counts reads.
    let mut per_call = Vec::new();
    for resource in &resources {
        let before = reads(&mut connection).await;
        harness
            .authorisation
            .decide(&subject, Action::Write, resource)
            .await
            .unwrap();
        per_call.push(reads(&mut connection).await - before);
    }
    let one_read = per_call[0];
    assert!(one_read > 0, "the view counts nothing");
    assert!(
        per_call.iter().all(|reads| *reads == one_read),
        "{per_call:?}"
    );

    let before = reads(&mut connection).await;
    harness
        .authorisation
        .decide_all(&subject, Action::Write, &resources)
        .await
        .unwrap();

    assert_eq!(reads(&mut connection).await - before, one_read);
}
