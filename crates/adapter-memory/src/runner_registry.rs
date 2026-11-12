use std::sync::Mutex;

use app::domain::name::OrganisationName;
use app::domain::principal::Principal;
use app::domain::runner::{NewRegistration, Registration, RegistrationId};
use app::port::driven::runner_registry::{RunnerRegistry, RunnerRegistryError};

use crate::lock;

#[derive(Default)]
struct Registrations {
    issued: u64,
    held: Vec<Registration>,
}

/// A runner registry in memory; it numbers registrations from 1 in registration order.
#[derive(Default)]
pub struct MemoryRunnerRegistry {
    registrations: Mutex<Registrations>,
}

impl RunnerRegistry for MemoryRunnerRegistry {
    async fn register(
        &self,
        registration: NewRegistration,
    ) -> Result<RegistrationId, RunnerRegistryError> {
        let mut registrations = lock(&self.registrations);
        registrations.issued = registrations.issued.saturating_add(1);
        let id = RegistrationId::new(registrations.issued);
        registrations.held.push(Registration {
            id,
            registered: registration,
        });
        Ok(id)
    }

    async fn remove(
        &self,
        organisation: &OrganisationName,
        id: RegistrationId,
    ) -> Result<(), RunnerRegistryError> {
        let mut registrations = lock(&self.registrations);
        let before = registrations.held.len();
        registrations
            .held
            .retain(|held| !(held.id == id && held.registered.organisation == *organisation));
        if registrations.held.len() == before {
            return Err(RunnerRegistryError::NotFound(id));
        }
        Ok(())
    }

    async fn list(
        &self,
        organisation: &OrganisationName,
    ) -> Result<Vec<Registration>, RunnerRegistryError> {
        Ok(lock(&self.registrations)
            .held
            .iter()
            .filter(|held| held.registered.organisation == *organisation)
            .cloned()
            .collect())
    }

    async fn of_principal(
        &self,
        principal: &Principal,
    ) -> Result<Vec<Registration>, RunnerRegistryError> {
        Ok(lock(&self.registrations)
            .held
            .iter()
            .filter(|held| held.registered.principal == *principal)
            .cloned()
            .collect())
    }
}
