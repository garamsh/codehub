use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use app::domain::name::OrganisationName;
use app::domain::principal::Principal;
use app::port::driven::membership::{Grant, Membership, MembershipError, Role};

use crate::authorisation_decision::MemoryAuthorisationDecision;
use crate::lock;

/// What a member holds in an organisation.
pub(crate) struct Member {
    pub(crate) role: Role,
    pub(crate) grants: BTreeSet<Grant>,
}

/// Every organisation's members; the authorisation decision evaluates it.
#[derive(Default)]
pub(crate) struct Members {
    pub(crate) members: BTreeMap<(OrganisationName, Principal), Member>,
}

/// Membership in memory.
#[derive(Default)]
pub struct MemoryMembership {
    state: Arc<Mutex<Members>>,
}

impl MemoryMembership {
    /// The authorisation decision over the roles and grants this membership holds.
    #[must_use]
    pub fn authorisation_decision(&self) -> MemoryAuthorisationDecision {
        MemoryAuthorisationDecision::new(Arc::clone(&self.state))
    }

    fn with_member<T>(
        &self,
        organisation: &OrganisationName,
        principal: &Principal,
        change: impl FnOnce(&mut Member) -> T,
    ) -> Result<T, MembershipError> {
        lock(&self.state)
            .members
            .get_mut(&(organisation.clone(), principal.clone()))
            .map(change)
            .ok_or_else(|| MembershipError::NotAMember(principal.clone()))
    }
}

impl Membership for MemoryMembership {
    async fn grant_role(
        &self,
        organisation: &OrganisationName,
        principal: &Principal,
        role: Role,
    ) -> Result<(), MembershipError> {
        lock(&self.state)
            .members
            .entry((organisation.clone(), principal.clone()))
            .and_modify(|member| member.role = role)
            .or_insert_with(|| Member {
                role,
                grants: BTreeSet::new(),
            });
        Ok(())
    }

    async fn revoke_role(
        &self,
        organisation: &OrganisationName,
        principal: &Principal,
    ) -> Result<(), MembershipError> {
        lock(&self.state)
            .members
            .remove(&(organisation.clone(), principal.clone()))
            .map(drop)
            .ok_or_else(|| MembershipError::NotAMember(principal.clone()))
    }

    async fn add_grant(
        &self,
        organisation: &OrganisationName,
        principal: &Principal,
        grant: Grant,
    ) -> Result<(), MembershipError> {
        self.with_member(organisation, principal, |member| {
            member.grants.insert(grant);
        })
    }

    async fn remove_grant(
        &self,
        organisation: &OrganisationName,
        principal: &Principal,
        grant: &Grant,
    ) -> Result<(), MembershipError> {
        self.with_member(organisation, principal, |member| {
            member.grants.remove(grant);
        })
    }
}
