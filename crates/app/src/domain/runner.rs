//! Runner registrations: the principals an organisation lets take checks, and which checks they
//! may take (ADR 0034).

use std::collections::BTreeSet;
use std::fmt;

use crate::domain::job::Job;
use crate::domain::name::{Label, OrganisationName};
use crate::domain::principal::Principal;
use crate::domain::repository::RepositoryId;

/// A registration: opaque, assigned by the runner registry.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RegistrationId(u64);

impl RegistrationId {
    /// The identifier a registry assigns from `value`; distinct values are distinct
    /// registrations.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The number the identifier was made from; [`RegistrationId::new`] of it is this
    /// identifier.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RegistrationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What a registration says before the registry has given it an identifier.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NewRegistration {
    /// The organisation whose checks the principal may take.
    pub organisation: OrganisationName,
    /// The one repository of the organisation it may take checks of; `None` for all of them.
    pub repository: Option<RepositoryId>,
    /// The runner.
    pub principal: Principal,
    /// What the runner offers; a check is taken only by a runner that offers all its labels.
    pub labels: BTreeSet<Label>,
}

/// A principal registered as a runner.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Registration {
    /// Names the registration.
    pub id: RegistrationId,
    /// What was registered.
    pub registered: NewRegistration,
}

impl Registration {
    /// Whether this registration may take `job`: the job is of its organisation and, where the
    /// registration names a repository, of that repository, and the runner offers every label the
    /// job's check requires.
    #[must_use]
    pub fn matches(&self, job: &Job) -> bool {
        let registered = &self.registered;
        registered.organisation == job.organisation
            && registered
                .repository
                .is_none_or(|repository| repository == job.repository)
            && job.check.labels.is_subset(&registered.labels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::address::Address;
    use crate::domain::check::{CheckDefinition, Command, Timeout};
    use crate::domain::job::JobState;

    fn job(organisation: &str, repository: u64, labels: &[&str]) -> Job {
        Job {
            id: crate::domain::job::JobId::new(1),
            organisation: organisation.parse().unwrap(),
            repository: RepositoryId::new(repository),
            changeset: Address::of(b"x"),
            check: CheckDefinition::new(
                "unit".parse().unwrap(),
                Command::new("true").unwrap(),
                labels.iter().map(|l| l.parse().unwrap()).collect(),
                true,
                Timeout::default_timeout(),
            ),
            state: JobState::Queued,
            runner: None,
            exit_code: None,
            output: Vec::new(),
            revert: None,
        }
    }

    fn registration(repository: Option<u64>, labels: &[&str]) -> Registration {
        Registration {
            id: RegistrationId::new(1),
            registered: NewRegistration {
                organisation: "acme".parse().unwrap(),
                repository: repository.map(RepositoryId::new),
                principal: Principal::new("https://id.test", "runner").unwrap(),
                labels: labels.iter().map(|l| l.parse().unwrap()).collect(),
            },
        }
    }

    #[test]
    fn a_registration_takes_jobs_of_its_organisation_whose_labels_it_offers() {
        let all = registration(None, &["linux", "gpu"]);
        assert!(all.matches(&job("acme", 1, &[])));
        assert!(all.matches(&job("acme", 2, &["linux"])));
        assert!(!all.matches(&job("acme", 2, &["linux", "arm"])));
        assert!(!all.matches(&job("other", 1, &[])));
    }

    #[test]
    fn a_registration_of_one_repository_takes_only_its_jobs() {
        let one = registration(Some(1), &[]);
        assert!(one.matches(&job("acme", 1, &[])));
        assert!(!one.matches(&job("acme", 2, &[])));
    }
}
