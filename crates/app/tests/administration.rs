//! Who administers members, an organisation created on first use, and the directory changes
//! reported, through the driving ports.

#[cfg(test)]
pub mod common;

use std::sync::Mutex;

use adapter_memory::MemoryRepositoryDirectory;
use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::path::{Path, Prefix};
use app::domain::principal::{Caller, Principal};
use app::domain::repository::RepositoryId;
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use app::port::driven::directory_changes::DirectoryChange;
use app::port::driven::membership::{Access, Role};
use app::port::driven::repository_directory::{DirectoryError, RepositoryDirectory};
use app::port::driving::members::{Members, MembersError};
use app::port::driving::repositories::{Repositories, RepositoriesError};
use app::port::driving::source_control::SourceControl;
use app::use_case::access::AccessError;
use app::use_case::administration::Administration;
use app::use_case::members::MembersService;
use app::use_case::repositories::RepositoriesService;
use app::use_case::source_control::SourceControlService;
use common::{Harness, MAX_UPLOAD_SIZE};

#[cfg(test)]
fn organisation(name: &str) -> OrganisationName {
    name.parse().unwrap()
}

#[cfg(test)]
fn app_repository() -> RepositoryName {
    "app".parse().unwrap()
}

#[cfg(test)]
fn organisation_created(name: &str) -> DirectoryChange {
    DirectoryChange::OrganisationCreated(organisation(name))
}

#[cfg(test)]
fn app_created(organisation: &str) -> DirectoryChange {
    DirectoryChange::RepositoryCreated {
        organisation: self::organisation(organisation),
        repository: app_repository(),
    }
}

/// Whether `caller` may create repositories in `organisation`, as the harness decides it.
#[cfg(test)]
async fn may_create_repositories(harness: &Harness, caller: &Caller, name: &str) -> Decision {
    harness
        .authorisation
        .decide(
            caller.principal(),
            Action::CreateRepository,
            &Resource::Organisation(organisation(name)),
        )
        .await
        .unwrap()
}

/// A decision point that cannot be reached.
#[cfg(test)]
struct Unreachable;

#[cfg(test)]
impl AuthorisationDecision for Unreachable {
    async fn decide(
        &self,
        _subject: &Principal,
        _action: Action,
        _resource: &Resource,
    ) -> Result<Decision, AuthorisationError> {
        Err(AuthorisationError::Backend("unreachable".to_owned()))
    }
}

/// A decision point that allows everything and keeps the resources it was asked about.
#[cfg(test)]
#[derive(Default)]
struct Recording {
    resources: Mutex<Vec<Resource>>,
}

#[cfg(test)]
impl AuthorisationDecision for Recording {
    async fn decide(
        &self,
        _subject: &Principal,
        _action: Action,
        resource: &Resource,
    ) -> Result<Decision, AuthorisationError> {
        self.resources.lock().unwrap().push(resource.clone());
        Ok(Decision::Allow)
    }
}

/// A directory in which another creator makes each organisation just before this one does.
#[cfg(test)]
#[derive(Default)]
struct Racing {
    inner: MemoryRepositoryDirectory,
}

#[cfg(test)]
impl RepositoryDirectory for Racing {
    async fn create_organisation(&self, name: &OrganisationName) -> Result<(), DirectoryError> {
        self.inner.create_organisation(name).await?;
        Err(DirectoryError::OrganisationExists(name.clone()))
    }

    async fn create_repository(
        &self,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> Result<RepositoryId, DirectoryError> {
        self.inner.create_repository(organisation, repository).await
    }

    async fn find_repository(
        &self,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> Result<RepositoryId, DirectoryError> {
        self.inner.find_repository(organisation, repository).await
    }

    async fn names_of(
        &self,
        repository: RepositoryId,
    ) -> Result<Option<(OrganisationName, RepositoryName)>, DirectoryError> {
        self.inner.names_of(repository).await
    }
}

/// Every members operation, as `caller` asking for `member`, over `authorisation`.
#[cfg(test)]
async fn every_members_operation<A: AuthorisationDecision + Sync>(
    harness: &Harness,
    authorisation: &A,
    administration: Administration,
    caller: &Caller,
    member: &Principal,
) -> Vec<Result<(), MembersError>> {
    let members = MembersService::new(
        &harness.directory,
        authorisation,
        &harness.membership,
        administration,
    );
    let (acme, app) = (organisation("acme"), app_repository());
    vec![
        members.add_member(caller, &acme, member, Role::Admin).await,
        members.remove_member(caller, &acme, member).await,
        members
            .add_grant(caller, &acme, member, &app, Prefix::whole(), Access::Read)
            .await,
        members
            .remove_grant(caller, &acme, member, &app, Prefix::whole(), Access::Read)
            .await,
    ]
}

#[tokio::test]
async fn refuses_every_members_operation_in_external_mode_before_asking_the_decision() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    let operation = |administration| {
        every_members_operation(
            &harness,
            &Unreachable,
            administration,
            &ada,
            bob.principal(),
        )
    };

    assert_eq!(
        operation(Administration::External).await,
        vec![Err(MembersError::ExternallyAdministered); 4]
    );
    // The same calls in built-in mode reach the decision, which is what fails them.
    let unreachable = MembersError::Access(AccessError::Authorisation(
        AuthorisationError::Backend("unreachable".to_owned()),
    ));
    assert_eq!(
        operation(Administration::BuiltIn).await,
        vec![Err(unreachable); 4]
    );
}

#[tokio::test]
async fn writes_no_member_when_refusing_a_members_operation_in_external_mode() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    harness.repository(&ada, "acme").await;
    let bob = harness.caller("bob").await;

    let refused = every_members_operation(
        &harness,
        &harness.authorisation,
        Administration::External,
        &ada,
        bob.principal(),
    )
    .await;

    assert_eq!(refused, vec![Err(MembersError::ExternallyAdministered); 4]);
    assert_eq!(
        may_create_repositories(&harness, &bob, "acme").await,
        Decision::Deny
    );
    // The same admin adding bob in built-in mode does write him, so the Deny above is the
    // refusal's.
    MembersService::new(
        &harness.directory,
        &harness.authorisation,
        &harness.membership,
        Administration::BuiltIn,
    )
    .add_member(&ada, &organisation("acme"), bob.principal(), Role::Admin)
    .await
    .unwrap();
    assert_eq!(
        may_create_repositories(&harness, &bob, "acme").await,
        Decision::Allow
    );
}

#[tokio::test]
async fn creates_an_organisation_in_external_mode_without_an_admin_grant_and_reports_it_once() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;

    harness
        .administered(Administration::External)
        .create_organisation(&ada, &organisation("acme"))
        .await
        .unwrap();

    assert_eq!(
        harness.changes.reported(),
        vec![organisation_created("acme")]
    );
    assert_eq!(
        may_create_repositories(&harness, &ada, "acme").await,
        Decision::Deny
    );
    // Built-in mode makes the creator an admin, so the Deny above is the mode's.
    harness
        .administered(Administration::BuiltIn)
        .create_organisation(&ada, &organisation("beta"))
        .await
        .unwrap();
    assert_eq!(
        may_create_repositories(&harness, &ada, "beta").await,
        Decision::Allow
    );
}

#[tokio::test]
async fn creates_a_missing_organisation_on_first_use_where_the_decision_allows_it() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    harness
        .authorisation
        .answer(Action::CreateRepository, Decision::Allow);
    harness
        .authorisation
        .answer(Action::CreateOrganisation, Decision::Allow);

    let id = harness
        .administered(Administration::External)
        .create_repository(&ada, &organisation("acme"), &app_repository())
        .await
        .unwrap();

    assert_eq!(
        harness
            .directory
            .find_repository(&organisation("acme"), &app_repository())
            .await,
        Ok(id)
    );
    assert_eq!(
        harness.changes.reported(),
        vec![organisation_created("acme"), app_created("acme")]
    );
}

#[tokio::test]
async fn does_not_create_a_missing_organisation_where_the_decision_denies_it() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    harness
        .authorisation
        .answer(Action::CreateRepository, Decision::Allow);
    harness
        .authorisation
        .answer(Action::CreateOrganisation, Decision::Deny);
    let repositories = harness.administered(Administration::External);

    assert_eq!(
        repositories
            .create_repository(&ada, &organisation("acme"), &app_repository())
            .await,
        Err(RepositoriesError::Directory(
            DirectoryError::OrganisationNotFound(organisation("acme"))
        ))
    );

    assert_eq!(
        harness
            .directory
            .create_repository(&organisation("acme"), &app_repository())
            .await,
        Err(DirectoryError::OrganisationNotFound(organisation("acme")))
    );
    assert_eq!(harness.changes.reported(), vec![]);
    // Allowing the creation is all that differs, so the refusal was the decision's.
    harness
        .authorisation
        .answer(Action::CreateOrganisation, Decision::Allow);
    repositories
        .create_repository(&ada, &organisation("acme"), &app_repository())
        .await
        .unwrap();
    assert_eq!(
        harness.changes.reported(),
        vec![organisation_created("acme"), app_created("acme")]
    );
}

#[tokio::test]
async fn counts_an_organisation_a_concurrent_creator_made_as_created_and_does_not_report_it() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    harness
        .authorisation
        .answer(Action::CreateRepository, Decision::Allow);
    let directory = Racing::default();
    let repositories = RepositoriesService::new(
        &directory,
        &harness.authorisation,
        &harness.membership,
        &harness.changes,
        Administration::External,
    );

    let id = repositories
        .create_repository(&ada, &organisation("acme"), &app_repository())
        .await
        .unwrap();

    assert_eq!(
        directory
            .find_repository(&organisation("acme"), &app_repository())
            .await,
        Ok(id)
    );
    assert_eq!(harness.changes.reported(), vec![app_created("acme")]);
}

#[tokio::test]
async fn does_not_create_a_missing_organisation_in_built_in_mode() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    harness
        .authorisation
        .answer(Action::CreateRepository, Decision::Allow);

    assert_eq!(
        harness
            .repositories()
            .create_repository(&ada, &organisation("acme"), &app_repository())
            .await,
        Err(RepositoriesError::Directory(
            DirectoryError::OrganisationNotFound(organisation("acme"))
        ))
    );

    assert_eq!(
        harness
            .directory
            .create_repository(&organisation("acme"), &app_repository())
            .await,
        Err(DirectoryError::OrganisationNotFound(organisation("acme")))
    );
    assert_eq!(harness.changes.reported(), vec![]);
}

#[tokio::test]
async fn reports_nothing_for_a_creation_that_fails() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    harness.repository(&ada, "acme").await;
    let both = vec![organisation_created("acme"), app_created("acme")];
    assert_eq!(harness.changes.reported(), both);
    let repositories = harness.repositories();

    assert_eq!(
        repositories
            .create_organisation(&ada, &organisation("acme"))
            .await,
        Err(RepositoriesError::Directory(
            DirectoryError::OrganisationExists(organisation("acme"))
        ))
    );
    assert_eq!(
        repositories
            .create_repository(&ada, &organisation("acme"), &app_repository())
            .await,
        Err(RepositoriesError::Directory(
            DirectoryError::RepositoryExists(organisation("acme"), app_repository())
        ))
    );
    assert_eq!(
        repositories
            .create_repository(&bob, &organisation("acme"), &"other".parse().unwrap())
            .await,
        Err(RepositoriesError::Access(AccessError::Denied(
            Action::CreateRepository
        )))
    );

    assert_eq!(harness.changes.reported(), both);
}

#[tokio::test]
async fn reports_a_repository_created_in_built_in_mode_and_makes_its_creator_admin() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;

    harness.repository(&ada, "acme").await;

    assert_eq!(
        harness.changes.reported(),
        vec![organisation_created("acme"), app_created("acme")]
    );
    assert_eq!(
        may_create_repositories(&harness, &ada, "acme").await,
        Decision::Allow
    );
}

#[tokio::test]
async fn creates_nothing_when_finding_a_repository_of_a_missing_organisation_in_external_mode() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    harness.authorisation.answer(Action::Read, Decision::Allow);
    harness
        .authorisation
        .answer(Action::CreateOrganisation, Decision::Allow);
    harness
        .authorisation
        .answer(Action::CreateRepository, Decision::Allow);

    assert_eq!(
        harness
            .administered(Administration::External)
            .find_repository(&ada, &organisation("acme"), &app_repository())
            .await,
        Err(RepositoriesError::Directory(
            DirectoryError::RepositoryNotFound(organisation("acme"), app_repository())
        ))
    );

    assert_eq!(
        harness
            .directory
            .create_repository(&organisation("acme"), &app_repository())
            .await,
        Err(DirectoryError::OrganisationNotFound(organisation("acme")))
    );
    assert_eq!(harness.changes.reported(), vec![]);
}

#[tokio::test]
async fn names_the_repository_in_each_resource_it_asks_the_decision_about() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    let id = harness.repository(&ada, "acme").await.id;
    let recording = Recording::default();
    let path: Path = "src/a.txt".parse().unwrap();

    RepositoriesService::new(
        &harness.directory,
        &recording,
        &harness.membership,
        &harness.changes,
        Administration::BuiltIn,
    )
    .find_repository(&ada, &organisation("acme"), &app_repository())
    .await
    .unwrap();
    // Whether the read succeeds is not the point; the decision has been asked by then.
    drop(
        SourceControlService::new(
            &harness.objects,
            &harness.log,
            &harness.derived,
            &harness.drafts,
            &harness.directory,
            &recording,
            MAX_UPLOAD_SIZE,
        )
        .read_file(&ada, id, &path)
        .await,
    );

    let repository = |organisation: &str| Resource::Repository {
        organisation: self::organisation(organisation),
        name: app_repository(),
        repository: id,
    };
    assert_eq!(
        *recording.resources.lock().unwrap(),
        vec![
            repository("acme"),
            repository("acme"),
            Resource::Path {
                organisation: organisation("acme"),
                name: app_repository(),
                repository: id,
                path,
            },
        ]
    );
}

#[tokio::test]
async fn answers_the_names_of_a_repository_from_its_identifier() {
    let harness = Harness::default();
    let ada = harness.caller("ada").await;
    let id = harness.repository(&ada, "acme").await.id;

    assert_eq!(
        harness.directory.names_of(id).await,
        Ok(Some((organisation("acme"), app_repository())))
    );
    assert_eq!(
        harness
            .directory
            .names_of(RepositoryId::new(id.value() + 1))
            .await,
        Ok(None)
    );
}
