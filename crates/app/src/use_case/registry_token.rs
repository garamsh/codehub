//! The registry token driving port, served by the directory, the authorisation decision and the
//! token issuance it is given (ADR 0053).

use std::sync::Arc;

use crate::domain::image::{ImageAction, Scope};
use crate::domain::principal::Caller;
use crate::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, Decision, Resource,
};
use crate::port::driven::registry_token_issuance::{GrantedEntry, RegistryTokenIssuance};
use crate::port::driven::repository_directory::{DirectoryError, RepositoryDirectory};
use crate::port::driving::registry_token::{
    MAX_SCOPES, RegistryToken, RegistryTokenError, RegistryTokenGrant, RegistryTokenRequest,
};

/// How long a token lasts, in seconds.
const LIFETIME: i64 = 300;

/// The driven ports the registry token is built over.
pub struct RegistryTokenAdapters {
    /// Looks repositories up.
    pub directory: Arc<dyn RepositoryDirectory>,
    /// Says what callers may do.
    pub authorisation: Arc<dyn AuthorisationDecision>,
    /// Signs the token.
    pub issuance: Arc<dyn RegistryTokenIssuance>,
}

/// The token service of one registry: grants a caller what the authorisation decision allows it
/// on the images of the repositories its scopes name.
pub struct RegistryTokenService {
    directory: Arc<dyn RepositoryDirectory>,
    authorisation: Arc<dyn AuthorisationDecision>,
    issuance: Arc<dyn RegistryTokenIssuance>,
    service: String,
    now: fn() -> i64,
}

impl RegistryTokenService {
    /// The token service of the registry named `service`, over `adapters`, reading the time in
    /// seconds since 1970-01-01T00:00:00Z from `now`.
    pub fn new(adapters: RegistryTokenAdapters, service: String, now: fn() -> i64) -> Self {
        let RegistryTokenAdapters {
            directory,
            authorisation,
            issuance,
        } = adapters;
        Self {
            directory,
            authorisation,
            issuance,
            service,
            now,
        }
    }

    /// The repository `scope` names, where the name is an image's and the directory holds it.
    async fn resource(&self, scope: &Scope) -> Result<Option<Resource>, DirectoryError> {
        let Some(image) = scope.image() else {
            return Ok(None);
        };
        match self
            .directory
            .find_repository(image.organisation(), image.repository())
            .await
        {
            Ok(repository) => Ok(Some(Resource::Repository {
                organisation: image.organisation().clone(),
                name: image.repository().clone(),
                repository,
            })),
            Err(
                DirectoryError::OrganisationNotFound(_) | DirectoryError::RepositoryNotFound(..),
            ) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The decisions of `caller` on `action` over `resources`; none where there is no resource.
    async fn decisions(
        &self,
        caller: &Caller,
        action: Action,
        resources: &[Resource],
    ) -> Result<Vec<Decision>, RegistryTokenError> {
        if resources.is_empty() {
            return Ok(Vec::new());
        }
        self.authorisation
            .decide_all(caller.principal(), action, resources)
            .await
            .map_err(RegistryTokenError::Authorisation)
    }
}

#[async_trait::async_trait]
impl RegistryToken for RegistryTokenService {
    async fn issue(
        &self,
        caller: &Caller,
        request: &RegistryTokenRequest,
    ) -> Result<RegistryTokenGrant, RegistryTokenError> {
        if request.service != self.service {
            return Err(RegistryTokenError::WrongService);
        }
        if request.scopes.len() > MAX_SCOPES {
            return Err(RegistryTokenError::TooManyScopes);
        }
        let scopes: Vec<Scope> = request
            .scopes
            .iter()
            .filter_map(|text| Scope::parse(text))
            .collect();
        let mut resources = Vec::new();
        let mut named = Vec::with_capacity(scopes.len());
        for scope in &scopes {
            let resource = self
                .resource(scope)
                .await
                .map_err(RegistryTokenError::Directory)?;
            named.push(resource.map(|resource| {
                resources.push(resource);
                resources.len() - 1
            }));
        }
        let pull = self
            .decisions(caller, Action::ImagePull, &resources)
            .await?;
        let push = self
            .decisions(caller, Action::ImagePush, &resources)
            .await?;
        let allowed =
            |decisions: &[Decision], at: usize| decisions.get(at) == Some(&Decision::Allow);
        let entries: Vec<GrantedEntry> = scopes
            .iter()
            .zip(&named)
            .map(|(scope, at)| {
                let mut actions = Vec::new();
                if let Some(at) = *at {
                    if scope.asks(ImageAction::Pull) && allowed(&pull, at) {
                        actions.push(ImageAction::Pull);
                    }
                    if scope.asks(ImageAction::Push) && allowed(&push, at) {
                        actions.push(ImageAction::Push);
                    }
                }
                GrantedEntry {
                    kind: scope.kind.clone(),
                    name: scope.name.clone(),
                    actions,
                }
            })
            .collect();
        let issued_at = (self.now)();
        let issued = self
            .issuance
            .issue(
                caller.principal().subject(),
                &self.service,
                &entries,
                issued_at,
                issued_at + LIFETIME,
            )
            .await
            .map_err(RegistryTokenError::Issuance)?;
        Ok(RegistryTokenGrant {
            token: issued.token,
            expires_in: issued.expires_in,
            issued_at: rfc3339(issued_at),
        })
    }
}

/// `seconds` since 1970-01-01T00:00:00Z as RFC 3339 text in UTC.
fn rfc3339(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        time % 3600 / 60,
        time % 60
    )
}

#[cfg(test)]
mod tests {
    use super::rfc3339;

    #[test]
    fn writes_the_time_as_rfc_3339_in_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_709_251_199), "2024-02-29T23:59:59Z");
        assert_eq!(rfc3339(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(rfc3339(-1), "1969-12-31T23:59:59Z");
    }
}
