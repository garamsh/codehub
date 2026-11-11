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
/// paths they cover, and a member holding any grant on a repository may read or write it as a
/// whole as that grant allows.
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
        (Access::Write, Action::Read | Action::Write) | (Access::Read, Action::Read) => true,
        (
            Access::Write | Access::Read,
            Action::CreateOrganisation | Action::CreateRepository | Action::AdministerMembers,
        )
        | (Access::Read, Action::Write) => false,
    }
}

/// The grants a member holds in the repository the resource is in.
type Grants = Vec<(Prefix, Access)>;

fn permits(role: Role, grants: &Grants, action: Action, resource: &Resource) -> bool {
    match (role, resource) {
        (Role::Admin, Resource::Organisation(_)) => {
            matches!(action, Action::CreateRepository | Action::AdministerMembers)
        }
        (Role::Admin, Resource::Repository { .. } | Resource::Path { .. }) => {
            matches!(action, Action::Read | Action::Write)
        }
        (Role::Member, Resource::Organisation(_)) => false,
        (Role::Member, Resource::Repository { .. }) => {
            grants.iter().any(|(_, access)| covers(*access, action))
        }
        (Role::Member, Resource::Path { path, .. }) => grants
            .iter()
            .any(|(prefix, access)| covers(*access, action) && prefix.covers(path)),
    }
}

impl AuthorisationDecision for PostgresAuthorisationDecision {
    async fn decide(
        &self,
        subject: &Principal,
        action: Action,
        resource: &Resource,
    ) -> Result<Decision, AuthorisationError> {
        if action == Action::CreateOrganisation {
            return Ok(match resource {
                Resource::Organisation(_) => Decision::Allow,
                Resource::Repository { .. } | Resource::Path { .. } => Decision::Deny,
            });
        }
        let (organisation, repository) = match resource {
            Resource::Organisation(organisation) => (organisation, None),
            Resource::Repository {
                organisation,
                repository,
            }
            | Resource::Path {
                organisation,
                repository,
                ..
            } => (organisation, Some(*repository)),
        };
        // An identifier outside the stored range was never assigned, so no grant is on it.
        let repository = repository.and_then(|id| conversion::repository_column(id).ok());
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
            return Ok(Decision::Deny);
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
        Ok(if permits(role, &grants, action, resource) {
            Decision::Allow
        } else {
            Decision::Deny
        })
    }
}
