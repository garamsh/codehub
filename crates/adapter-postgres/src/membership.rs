use app::domain::name::OrganisationName;
use app::domain::principal::Principal;
use app::port::driven::membership::{Grant, Membership, MembershipError, Role};
use sqlx::PgPool;

use crate::conversion;
use crate::database::Database;

/// Membership in PostgreSQL.
pub struct PostgresMembership {
    pool: PgPool,
}

impl PostgresMembership {
    /// Membership over `database`.
    #[must_use]
    pub fn new(database: &Database) -> Self {
        Self {
            pool: database.pool.clone(),
        }
    }
}

fn backend(error: impl ToString) -> MembershipError {
    MembershipError::Backend(error.to_string())
}

fn repository_column(grant: &Grant) -> Result<i64, MembershipError> {
    conversion::repository_column(grant.repository).map_err(MembershipError::Backend)
}

impl Membership for PostgresMembership {
    async fn grant_role(
        &self,
        organisation: &OrganisationName,
        principal: &Principal,
        role: Role,
    ) -> Result<(), MembershipError> {
        sqlx::query(
            "insert into members (organisation, issuer, subject, role) values ($1, $2, $3, $4) \
             on conflict (organisation, issuer, subject) do update set role = excluded.role",
        )
        .bind(organisation.as_str())
        .bind(principal.issuer())
        .bind(principal.subject())
        .bind(conversion::role_column(role))
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn revoke_role(
        &self,
        organisation: &OrganisationName,
        principal: &Principal,
    ) -> Result<(), MembershipError> {
        let removed = sqlx::query(
            "delete from members where organisation = $1 and issuer = $2 and subject = $3",
        )
        .bind(organisation.as_str())
        .bind(principal.issuer())
        .bind(principal.subject())
        .execute(&self.pool)
        .await
        .map_err(backend)?
        .rows_affected();
        if removed == 0 {
            return Err(MembershipError::NotAMember(principal.clone()));
        }
        Ok(())
    }

    async fn add_grant(
        &self,
        organisation: &OrganisationName,
        principal: &Principal,
        grant: Grant,
    ) -> Result<(), MembershipError> {
        sqlx::query(
            "insert into grants (organisation, issuer, subject, repository, prefix, access) \
             values ($1, $2, $3, $4, $5, $6) on conflict do nothing",
        )
        .bind(organisation.as_str())
        .bind(principal.issuer())
        .bind(principal.subject())
        .bind(repository_column(&grant)?)
        .bind(conversion::prefix_column(&grant.prefix))
        .bind(conversion::access_column(grant.access))
        .execute(&self.pool)
        .await
        .map_err(|error| {
            let not_a_member = error.as_database_error().is_some_and(|error| {
                error.is_foreign_key_violation() && error.constraint() == Some("grants_member")
            });
            if not_a_member {
                MembershipError::NotAMember(principal.clone())
            } else {
                backend(error)
            }
        })?;
        Ok(())
    }

    async fn remove_grant(
        &self,
        organisation: &OrganisationName,
        principal: &Principal,
        grant: &Grant,
    ) -> Result<(), MembershipError> {
        // The delete runs whether or not the member exists; the select reports which.
        let member = sqlx::query_scalar::<_, bool>(
            "with removed as ( \
                 delete from grants \
                 where organisation = $1 and issuer = $2 and subject = $3 \
                 and repository = $4 and prefix = $5 and access = $6 \
             ) \
             select exists ( \
                 select 1 from members \
                 where organisation = $1 and issuer = $2 and subject = $3 \
             )",
        )
        .bind(organisation.as_str())
        .bind(principal.issuer())
        .bind(principal.subject())
        .bind(repository_column(grant)?)
        .bind(conversion::prefix_column(&grant.prefix))
        .bind(conversion::access_column(grant.access))
        .fetch_one(&self.pool)
        .await
        .map_err(backend)?;
        if !member {
            return Err(MembershipError::NotAMember(principal.clone()));
        }
        Ok(())
    }
}
