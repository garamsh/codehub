use std::collections::HashMap;
use std::collections::hash_map::Entry;

use app::domain::name::OrganisationName;
use app::domain::path::Prefix;
use app::domain::principal::Principal;
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use app::port::driven::membership::{Access, Role};
use sqlx::PgPool;

use crate::conversion;
use crate::database::Database;

/// The ADR 0021 policy evaluated over the memberships of [`crate::PostgresMembership`]: an admin
/// may take every action in its organisation; a member's grants allow `read` and `write` on the
/// paths they cover, and a member holding any grant on a repository may read, write or run it
/// as a whole as that grant allows; `run` is asked of a repository only (ADR 0051).
pub struct PostgresAuthorisationDecision {
    pool: PgPool,
}

impl PostgresAuthorisationDecision {
    /// The authorisation decision over `database`.
    #[must_use]
    pub fn new(database: &Database) -> Self {
        Self {
            pool: database.pool.clone(),
        }
    }
}

fn backend(error: impl ToString) -> AuthorisationError {
    AuthorisationError::Backend(error.to_string())
}

const fn covers(held: Access, asked: Action) -> bool {
    match (held, asked) {
        (Access::Write, Action::Read | Action::Write)
        | (Access::Read, Action::Read)
        | (Access::Run, Action::Run) => true,
        (
            Access::Write | Access::Read | Access::Run,
            Action::CreateOrganisation
            | Action::CreateRepository
            | Action::AdministerMembers
            | Action::AdministerRunners,
        )
        | (Access::Read, Action::Write | Action::Run)
        | (Access::Write, Action::Run)
        | (Access::Run, Action::Read | Action::Write) => false,
    }
}

/// The grants a member holds in the repository the resource is in.
type Grants = Vec<(Prefix, Access)>;

fn permits(role: Role, grants: &Grants, action: Action, resource: &Resource) -> bool {
    match (role, resource) {
        (Role::Admin, Resource::Organisation(_)) => {
            matches!(
                action,
                Action::CreateRepository | Action::AdministerMembers | Action::AdministerRunners
            )
        }
        (Role::Admin, Resource::Repository { .. }) => {
            matches!(action, Action::Read | Action::Write | Action::Run)
        }
        (Role::Admin, Resource::Path { .. }) => matches!(action, Action::Read | Action::Write),
        (Role::Member, Resource::Organisation(_)) => false,
        (Role::Member, Resource::Repository { .. }) => {
            grants.iter().any(|(_, access)| covers(*access, action))
        }
        (Role::Member, Resource::Path { path, .. }) => {
            action != Action::Run
                && grants
                    .iter()
                    .any(|(prefix, access)| covers(*access, action) && prefix.covers(path))
        }
    }
}

/// What a member holds in one repository, or in none: its role and grants, or nothing for a
/// principal that is no member.
type Held = Option<(Role, Grants)>;

/// The organisation a resource is in and, for one in a repository, the repository's stored
/// identifier. An identifier outside the stored range was never assigned, so no grant is on it.
fn scope(resource: &Resource) -> (&OrganisationName, Option<i64>) {
    match resource {
        Resource::Organisation(organisation) => (organisation, None),
        Resource::Repository {
            organisation,
            repository,
            ..
        }
        | Resource::Path {
            organisation,
            repository,
            ..
        } => (
            organisation,
            conversion::repository_column(*repository).ok(),
        ),
    }
}

/// The decision on `action` for a resource that is not asked of a member's holdings, if it is one.
const fn unconditional(action: Action, resource: &Resource) -> Option<Decision> {
    if !matches!(action, Action::CreateOrganisation) {
        return None;
    }
    Some(match resource {
        Resource::Organisation(_) => Decision::Allow,
        Resource::Repository { .. } | Resource::Path { .. } => Decision::Deny,
    })
}

fn verdict(held: &Held, action: Action, resource: &Resource) -> Decision {
    match held {
        Some((role, grants)) if permits(*role, grants, action, resource) => Decision::Allow,
        Some(_) | None => Decision::Deny,
    }
}

impl PostgresAuthorisationDecision {
    /// The role `subject` holds in `organisation` and its grants in `repository`.
    async fn held(
        &self,
        subject: &Principal,
        organisation: &OrganisationName,
        repository: Option<i64>,
    ) -> Result<Held, AuthorisationError> {
        // One statement reads the role and the grants from one snapshot; a member with no grants
        // there yields its role alone, with null grant columns.
        let rows = sqlx::query_as::<_, (String, Option<Vec<u8>>, Option<String>)>(
            "select m.role, g.prefix, g.access from members m \
             left join grants g \
             on g.organisation = m.organisation and g.issuer = m.issuer \
             and g.subject = m.subject and g.repository = $4 \
             where m.organisation = $1 and m.issuer = $2 and m.subject = $3",
        )
        .bind(organisation.as_str())
        .bind(subject.issuer())
        .bind(subject.subject())
        .bind(repository)
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        let Some((role, ..)) = rows.first() else {
            return Ok(None);
        };
        let role = conversion::role(role).map_err(AuthorisationError::Backend)?;
        let mut grants = Grants::new();
        for (_, prefix, access) in &rows {
            if let (Some(prefix), Some(access)) = (prefix, access) {
                grants.push((
                    conversion::prefix(prefix).map_err(AuthorisationError::Backend)?,
                    conversion::access(access).map_err(AuthorisationError::Backend)?,
                ));
            }
        }
        Ok(Some((role, grants)))
    }
}

#[async_trait::async_trait]
impl AuthorisationDecision for PostgresAuthorisationDecision {
    async fn decide(
        &self,
        subject: &Principal,
        action: Action,
        resource: &Resource,
    ) -> Result<Decision, AuthorisationError> {
        if let Some(decision) = unconditional(action, resource) {
            return Ok(decision);
        }
        let (organisation, repository) = scope(resource);
        let held = self.held(subject, organisation, repository).await?;
        Ok(verdict(&held, action, resource))
    }

    /// Reads what the member holds once for each repository the resources are in, so a batch in
    /// one repository costs one query however many paths it carries.
    async fn decide_all(
        &self,
        subject: &Principal,
        action: Action,
        resources: &[Resource],
    ) -> Result<Vec<Decision>, AuthorisationError> {
        let mut read: HashMap<(&str, Option<i64>), Held> = HashMap::new();
        let mut decisions = Vec::with_capacity(resources.len());
        for resource in resources {
            if let Some(decision) = unconditional(action, resource) {
                decisions.push(decision);
                continue;
            }
            let (organisation, repository) = scope(resource);
            let key = (organisation.as_str(), repository);
            let held = match read.entry(key) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    entry.insert(self.held(subject, organisation, repository).await?)
                }
            };
            decisions.push(verdict(held, action, resource));
        }
        Ok(decisions)
    }
}
