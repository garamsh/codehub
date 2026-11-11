//! Authenticating callers and authorising them per path, through the driving ports.

#[cfg(test)]
pub mod common;

use std::collections::BTreeSet;

use app::domain::address::Address;
use app::domain::changeset::Change;
use app::domain::file::{File, FileKind};
use app::domain::name::OrganisationName;
use app::domain::path::{Path, Prefix};
use app::domain::principal::{Caller, Principal};
use app::domain::repository::RepositoryId;
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use app::port::driven::identity_verification::IdentityError;
use app::port::driven::membership::{Access, Role};
use app::port::driven::repository_directory::DirectoryError;
use app::port::driving::authentication::{Authentication, AuthenticationError};
use app::port::driving::members::Members;
use app::port::driving::repositories::{Repositories, RepositoriesError};
use app::port::driving::source_control::{SourceControl, SourceControlError};
use app::use_case::access::AccessError;
use app::use_case::administration::Administration;
use app::use_case::land::LandError;
use app::use_case::members::MembersService;
use app::use_case::source_control::SourceControlService;
use common::{Harness, MAX_UPLOAD_SIZE, Repo, draft};

/// A prefix over the path `path`, whole components only; empty is the whole repository.
#[cfg(test)]
fn prefix(path: &str) -> Prefix {
    if path.is_empty() {
        Prefix::whole()
    } else {
        Prefix::from(path.parse::<Path>().unwrap())
    }
}

#[cfg(test)]
fn acme() -> OrganisationName {
    "acme".parse().unwrap()
}

/// The members port over the harness's adapters, administered by `admin` in `acme`.
#[cfg(test)]
struct Admin<'a> {
    harness: &'a Harness,
    caller: &'a Caller,
}

#[cfg(test)]
impl Admin<'_> {
    async fn add(&self, member: &Caller, role: Role) {
        MembersService::new(
            &self.harness.directory,
            &self.harness.authorisation,
            &self.harness.membership,
            Administration::BuiltIn,
        )
        .add_member(self.caller, &acme(), member.principal(), role)
        .await
        .unwrap();
    }

    async fn remove(&self, member: &Caller) {
        MembersService::new(
            &self.harness.directory,
            &self.harness.authorisation,
            &self.harness.membership,
            Administration::BuiltIn,
        )
        .remove_member(self.caller, &acme(), member.principal())
        .await
        .unwrap();
    }

    async fn grant(&self, member: &Caller, path: &str, access: Access) {
        MembersService::new(
            &self.harness.directory,
            &self.harness.authorisation,
            &self.harness.membership,
            Administration::BuiltIn,
        )
        .add_grant(
            self.caller,
            &acme(),
            member.principal(),
            &"app".parse().unwrap(),
            prefix(path),
            access,
        )
        .await
        .unwrap();
    }

    async fn ungrant(&self, member: &Caller, path: &str, access: Access) {
        MembersService::new(
            &self.harness.directory,
            &self.harness.authorisation,
            &self.harness.membership,
            Administration::BuiltIn,
        )
        .remove_grant(
            self.caller,
            &acme(),
            member.principal(),
            &"app".parse().unwrap(),
            prefix(path),
            access,
        )
        .await
        .unwrap();
    }
}

/// Stages a regular file `content` at `path` in `caller`'s draft `name`, through the port.
#[cfg(test)]
async fn try_stage(
    harness: &Harness,
    repo: &Repo<'_>,
    caller: &Caller,
    name: &str,
    path: &str,
    content: &str,
) -> Result<(), SourceControlError> {
    let content: Address = repo.upload(content).await;
    harness
        .source_control()
        .stage(
            caller,
            repo.id,
            &draft(name),
            path.parse().unwrap(),
            Change::File(File {
                kind: FileKind::Regular,
                content,
            }),
        )
        .await
}

#[cfg(test)]
fn denied_paths(access: Access, paths: &[&str]) -> SourceControlError {
    SourceControlError::Access(AccessError::PathsDenied {
        access,
        paths: paths
            .iter()
            .map(|path| path.parse().unwrap())
            .collect::<BTreeSet<Path>>(),
    })
}

/// What landing reports of a draft changing paths its author may not write.
#[cfg(test)]
fn land_denied(paths: &[&str]) -> SourceControlError {
    SourceControlError::Land(LandError::Access(AccessError::PathsDenied {
        access: Access::Write,
        paths: paths.iter().map(|path| path.parse().unwrap()).collect(),
    }))
}

#[tokio::test]
async fn an_invalid_credential_is_rejected() {
    let harness = Harness::new().await;
    let authentication = harness.authentication();
    let unknown = Harness::credential("mallory");

    assert_eq!(
        authentication.authenticate(&unknown).await.err(),
        Some(AuthenticationError::Identity(IdentityError::Rejected))
    );

    // Control: the same credential authenticates once it identifies a principal.
    harness
        .identity
        .register(&unknown, Harness::principal("mallory"));
    let caller = authentication.authenticate(&unknown).await.unwrap();
    assert_eq!(caller.principal(), &Harness::principal("mallory"));
}

#[tokio::test]
async fn any_caller_creates_an_organisation_and_becomes_its_admin() {
    let harness = Harness::new().await;
    let bob = harness.caller("bob").await;
    let eve = harness.caller("eve").await;
    let repositories = harness.repositories();
    let app = "app".parse().unwrap();

    repositories
        .create_organisation(&bob, &acme())
        .await
        .unwrap();

    // The creator administers it: it may create a repository there.
    assert!(
        repositories
            .create_repository(&bob, &acme(), &app)
            .await
            .is_ok()
    );
    // Control: another authenticated caller, no member, may not.
    assert_eq!(
        repositories
            .create_repository(&eve, &acme(), &"other".parse().unwrap())
            .await,
        Err(RepositoriesError::Access(AccessError::Denied(
            Action::CreateRepository
        )))
    );
}

#[tokio::test]
async fn a_non_admin_cannot_create_a_repository() {
    let harness = Harness::new().await;
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    let repo = harness.repository(&ada, "acme").await;
    let admin = Admin {
        harness: &harness,
        caller: &ada,
    };
    admin.add(&bob, Role::Member).await;
    admin.grant(&bob, "", Access::Write).await;
    let second = "second".parse().unwrap();

    assert_eq!(
        harness
            .repositories()
            .create_repository(&bob, &acme(), &second)
            .await,
        Err(RepositoriesError::Access(AccessError::Denied(
            Action::CreateRepository
        )))
    );
    assert_eq!(
        harness
            .repositories()
            .find_repository(&bob, &acme(), &second)
            .await,
        Err(RepositoriesError::Directory(
            DirectoryError::RepositoryNotFound(acme(), second.clone())
        ))
    );

    // Control: the admin creates it, and the member holding a grant on the first reads it.
    assert!(
        harness
            .repositories()
            .create_repository(&ada, &acme(), &second)
            .await
            .is_ok()
    );
    assert_eq!(
        harness
            .repositories()
            .find_repository(&bob, &acme(), &"app".parse().unwrap())
            .await,
        Ok(repo.id)
    );
}

#[tokio::test]
async fn a_member_with_write_on_a_prefix_lands_inside_it_and_a_draft_beyond_it_is_rejected() {
    let harness = Harness::new().await;
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    let repo = harness.repository(&ada, "acme").await;
    let admin = Admin {
        harness: &harness,
        caller: &ada,
    };
    admin.add(&bob, Role::Member).await;
    admin.grant(&bob, "src", Access::Write).await;

    // Inside the prefix lands.
    repo.stage(&bob, &draft("inside"), &[("src/a", "a\n")])
        .await;
    assert!(repo.land(&bob, &draft("inside"), "inside").await.is_ok());
    assert_eq!(repo.read("src/a").await.as_deref(), Some("a\n"));

    // Whole components only: `srcx` is not under `src`, and nor is `docs`.
    assert_eq!(
        try_stage(&harness, &repo, &bob, "beyond", "srcx/a", "x").await,
        Err(denied_paths(Access::Write, &["srcx/a"]))
    );
    assert_eq!(
        try_stage(&harness, &repo, &bob, "beyond", "docs/b", "x").await,
        Err(denied_paths(Access::Write, &["docs/b"]))
    );

    // A draft changing both: the grant was wider when it was staged and has narrowed since.
    admin.grant(&bob, "", Access::Write).await;
    repo.stage(
        &bob,
        &draft("both"),
        &[("src/a", "a2\n"), ("docs/b", "b\n"), ("docs/c", "c\n")],
    )
    .await;
    admin.ungrant(&bob, "", Access::Write).await;
    let main = repo.main().await;

    assert_eq!(
        repo.land(&bob, &draft("both"), "both").await,
        Err(land_denied(&["docs/b", "docs/c"]))
    );
    assert_eq!(repo.main().await, main);
    assert_eq!(repo.read("src/a").await.as_deref(), Some("a\n"));
    assert_eq!(repo.read("docs/b").await, None);

    // Control: the same draft lands once the grant covers every path it changes.
    admin.grant(&bob, "docs", Access::Write).await;
    assert!(repo.land(&bob, &draft("both"), "both").await.is_ok());
    assert_eq!(repo.read("docs/b").await.as_deref(), Some("b\n"));
}

#[tokio::test]
async fn a_read_only_member_cannot_land() {
    let harness = Harness::new().await;
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    let repo = harness.repository(&ada, "acme").await;
    let admin = Admin {
        harness: &harness,
        caller: &ada,
    };
    admin.add(&bob, Role::Member).await;
    admin.grant(&bob, "", Access::Write).await;
    repo.stage(&bob, &draft("work"), &[("a", "a\n")]).await;
    admin.ungrant(&bob, "", Access::Write).await;
    admin.grant(&bob, "", Access::Read).await;

    assert_eq!(
        repo.land(&bob, &draft("work"), "work").await,
        Err(land_denied(&["a"]))
    );
    assert_eq!(
        try_stage(&harness, &repo, &bob, "more", "b", "b").await,
        Err(denied_paths(Access::Write, &["b"]))
    );
    assert_eq!(repo.main().await, None);

    // Control: reading is allowed, and the write grant lands the draft.
    let path: Path = "a".parse().unwrap();
    assert_eq!(
        harness
            .source_control()
            .history(&bob, repo.id, None, &path)
            .await,
        Ok(Vec::new())
    );
    admin.grant(&bob, "", Access::Write).await;
    assert!(repo.land(&bob, &draft("work"), "work").await.is_ok());
    assert_eq!(repo.read("a").await.as_deref(), Some("a\n"));
}

#[tokio::test]
async fn a_non_member_reads_and_asks_for_history_as_of_a_missing_repository() {
    let harness = Harness::new().await;
    let ada = harness.caller("ada").await;
    let eve = harness.caller("eve").await;
    let repo = harness.repository(&ada, "acme").await;
    repo.stage(&ada, &draft("seed"), &[("a", "a\n")]).await;
    repo.land(&ada, &draft("seed"), "seed").await.unwrap();
    let source_control = harness.source_control();
    let path: Path = "a".parse().unwrap();
    let missing = RepositoryId::new(u64::MAX);
    let not_found = |id| SourceControlError::Access(AccessError::RepositoryNotFound(id));

    // Control: the admin reads and sees the history.
    assert!(source_control.read_file(&ada, repo.id, &path).await.is_ok());
    assert_eq!(
        source_control
            .history(&ada, repo.id, None, &path)
            .await
            .map(|history| history.len()),
        Ok(1)
    );

    assert_eq!(
        source_control.read_file(&eve, repo.id, &path).await.err(),
        Some(not_found(repo.id))
    );
    assert_eq!(
        source_control.history(&eve, repo.id, None, &path).await,
        Err(not_found(repo.id))
    );
    // Nothing else tells it from a repository that does not exist.
    assert_eq!(
        source_control.read_file(&ada, missing, &path).await.err(),
        Some(not_found(missing))
    );
    assert_eq!(
        source_control.history(&ada, missing, None, &path).await,
        Err(not_found(missing))
    );
    assert_eq!(
        harness
            .repositories()
            .find_repository(&eve, &acme(), &"app".parse().unwrap())
            .await,
        Err(RepositoriesError::Directory(
            DirectoryError::RepositoryNotFound(acme(), "app".parse().unwrap())
        ))
    );
    assert_eq!(
        try_stage(&harness, &repo, &eve, "work", "a", "x").await,
        Err(not_found(repo.id))
    );
}

#[tokio::test]
async fn a_removed_member_loses_access() {
    let harness = Harness::new().await;
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    let repo = harness.repository(&ada, "acme").await;
    let admin = Admin {
        harness: &harness,
        caller: &ada,
    };
    admin.add(&bob, Role::Member).await;
    admin.grant(&bob, "", Access::Write).await;
    repo.stage(&bob, &draft("work"), &[("a", "a\n")]).await;
    let path: Path = "a".parse().unwrap();
    let source_control = harness.source_control();
    assert!(
        harness
            .repositories()
            .find_repository(&bob, &acme(), &"app".parse().unwrap())
            .await
            .is_ok()
    );

    admin.remove(&bob).await;

    assert_eq!(
        repo.land(&bob, &draft("work"), "work").await,
        Err(SourceControlError::Access(AccessError::RepositoryNotFound(
            repo.id
        )))
    );
    assert_eq!(
        source_control.history(&bob, repo.id, None, &path).await,
        Err(SourceControlError::Access(AccessError::RepositoryNotFound(
            repo.id
        )))
    );
    // Control: the admin is unaffected, and re-adding the member does not restore its grants.
    assert!(
        source_control
            .history(&ada, repo.id, None, &path)
            .await
            .is_ok()
    );
    admin.add(&bob, Role::Member).await;
    assert_eq!(
        source_control.history(&bob, repo.id, None, &path).await,
        Err(SourceControlError::Access(AccessError::RepositoryNotFound(
            repo.id
        )))
    );
}

/// An authorisation decision whose backend is down.
#[cfg(test)]
struct Down;

#[cfg(test)]
impl AuthorisationDecision for Down {
    async fn decide(
        &self,
        _subject: &Principal,
        _action: Action,
        _resource: &Resource,
    ) -> Result<Decision, AuthorisationError> {
        Err(AuthorisationError::Backend("down".to_owned()))
    }
}

#[tokio::test]
async fn a_failing_authorisation_backend_refuses_the_call_with_its_description() {
    let harness = Harness::new().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    repo.stage(&ada, &draft("seed"), &[("a", "a\n")]).await;
    repo.land(&ada, &draft("seed"), "seed").await.unwrap();
    let path: Path = "a".parse().unwrap();
    let down = SourceControlService::new(
        &harness.objects,
        &harness.log,
        &harness.derived,
        &harness.drafts,
        &harness.directory,
        &Down,
        MAX_UPLOAD_SIZE,
    );

    assert_eq!(
        down.read_file(&ada, repo.id, &path).await.err(),
        Some(SourceControlError::Access(AccessError::Authorisation(
            AuthorisationError::Backend("down".to_owned())
        )))
    );
    // Control: the same call over the working policy succeeds.
    assert!(
        harness
            .source_control()
            .read_file(&ada, repo.id, &path)
            .await
            .is_ok()
    );
}
