use std::sync::{Arc, Mutex};

use app::domain::principal::Principal;
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use app::port::driven::membership::{Access, Role};

use crate::lock;
use crate::membership::{Member, Members};

/// The ADR 0021 policy evaluated over the memberships of a [`crate::MemoryMembership`]: an admin
/// may take every action in its organisation; a member's grants allow `read` and `write` on the
/// paths they cover, and a member holding any grant on a repository may read or write it as a
/// whole as that grant allows. [`MemoryAuthorisationDecision::answer`] fixes the decision for an
/// action ahead of that policy, as an outside decision point that keeps its own entities would.
pub struct MemoryAuthorisationDecision {
    state: Arc<Mutex<Members>>,
    answers: Mutex<Vec<(Action, Decision)>>,
}

impl MemoryAuthorisationDecision {
    pub(crate) const fn new(state: Arc<Mutex<Members>>) -> Self {
        Self {
            state,
            answers: Mutex::new(Vec::new()),
        }
    }

    /// From now on decides `action` as `decision`, on every resource and for every subject,
    /// replacing an answer given before for it.
    pub fn answer(&self, action: Action, decision: Decision) {
        let mut answers = lock(&self.answers);
        answers.retain(|(answered, _)| *answered != action);
        answers.push((action, decision));
    }
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

fn permits(member: &Member, action: Action, resource: &Resource) -> bool {
    match (member.role, resource) {
        (Role::Admin, Resource::Organisation(_)) => {
            matches!(action, Action::CreateRepository | Action::AdministerMembers)
        }
        (Role::Admin, Resource::Repository { .. } | Resource::Path { .. }) => {
            matches!(action, Action::Read | Action::Write)
        }
        (Role::Member, Resource::Organisation(_)) => false,
        (Role::Member, Resource::Repository { repository, .. }) => member
            .grants
            .iter()
            .any(|grant| grant.repository == *repository && covers(grant.access, action)),
        (
            Role::Member,
            Resource::Path {
                repository, path, ..
            },
        ) => member.grants.iter().any(|grant| {
            grant.repository == *repository
                && covers(grant.access, action)
                && grant.prefix.covers(path)
        }),
    }
}

impl AuthorisationDecision for MemoryAuthorisationDecision {
    async fn decide(
        &self,
        subject: &Principal,
        action: Action,
        resource: &Resource,
    ) -> Result<Decision, AuthorisationError> {
        let answered = lock(&self.answers)
            .iter()
            .find(|(answered, _)| *answered == action)
            .map(|(_, decision)| *decision);
        if let Some(decision) = answered {
            return Ok(decision);
        }
        if action == Action::CreateOrganisation {
            return Ok(match resource {
                Resource::Organisation(_) => Decision::Allow,
                Resource::Repository { .. } | Resource::Path { .. } => Decision::Deny,
            });
        }
        let organisation = match resource {
            Resource::Organisation(organisation)
            | Resource::Repository { organisation, .. }
            | Resource::Path { organisation, .. } => organisation,
        };
        let allowed = lock(&self.state)
            .members
            .get(&(organisation.clone(), subject.clone()))
            .is_some_and(|member| permits(member, action, resource));
        Ok(if allowed {
            Decision::Allow
        } else {
            Decision::Deny
        })
    }
}
