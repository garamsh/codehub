//! The one assembly of the driving ports from the driven ports, which the composition site and
//! every harness that serves the application build the shipped graph through.

use std::sync::Arc;

use crate::port::driven::authorisation_decision::AuthorisationDecision;
use crate::port::driven::cache_refs::CacheRefs;
use crate::port::driven::changeset_log::ChangesetLog;
use crate::port::driven::check_definitions::CheckDefinitions;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::directory_changes::DirectoryChanges;
use crate::port::driven::draft_store::DraftStore;
use crate::port::driven::identity_verification::IdentityVerification;
use crate::port::driven::job_queue::JobQueue;
use crate::port::driven::membership::Membership;
use crate::port::driven::object_store::ObjectStore;
use crate::port::driven::registry_token_issuance::RegistryTokenIssuance;
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driven::runner_registry::RunnerRegistry;
use crate::use_case::administration::Administration;
use crate::use_case::authentication::AuthenticationService;
use crate::use_case::checks::{ChecksAdapters, ChecksService};
use crate::use_case::jobs::{JobsAdapters, JobsService};
use crate::use_case::landing_attempts::LandingAttempts;
use crate::use_case::members::{MembersAdapters, MembersService};
use crate::use_case::post_land_checks::{PostLandChecks, PostLandChecksAdapters};
use crate::use_case::registry_token::{RegistryTokenAdapters, RegistryTokenService};
use crate::use_case::relay::Delivery;
use crate::use_case::repositories::{RepositoriesAdapters, RepositoriesService};
use crate::use_case::runners::{RunnersAdapters, RunnersService};
use crate::use_case::runs::{RunsAdapters, RunsService};
use crate::use_case::source_control::{SourceControlAdapters, SourceControlService};

/// The driving ports as the application ships them, over the driven ports it is given: source
/// control queues the checks of each landing, and the jobs and the checks revert a changeset
/// whose blocking check failed. Its fields are what the driving ports are built over.
pub struct Assembly {
    /// Stores blobs.
    pub objects: Arc<dyn ObjectStore>,
    /// Stores changesets and the refs.
    pub log: Arc<dyn ChangesetLog>,
    /// Stores derived data.
    pub derived: Arc<dyn DerivedData>,
    /// Stores drafts.
    pub drafts: Arc<dyn DraftStore>,
    /// Keeps organisations and repositories.
    pub directory: Arc<dyn RepositoryDirectory>,
    /// Says what callers may do.
    pub authorisation: Arc<dyn AuthorisationDecision>,
    /// Keeps members and their grants.
    pub membership: Arc<dyn Membership>,
    /// Holds the jobs.
    pub queue: Arc<dyn JobQueue>,
    /// Holds the cache refs.
    pub cache_refs: Arc<dyn CacheRefs>,
    /// Finds the callers' registrations.
    pub registry: Arc<dyn RunnerRegistry>,
    /// Reads the check definitions.
    pub definitions: Arc<dyn CheckDefinitions>,
    /// Receives each organisation creation.
    pub changes: Arc<dyn DirectoryChanges>,
    /// Delivers the entries of a repository's creation once it has committed.
    pub delivery: Arc<dyn Delivery>,
    /// Verifies credentials.
    pub identity: Arc<dyn IdentityVerification>,
    /// Whether members are kept here or in an external directory.
    pub administration: Administration,
    /// The largest upload source control grants, in bytes.
    pub max_upload_size: u64,
    /// How many times a landing or a revert that lost the compare-and-swap on `main` is tried.
    pub attempts: LandingAttempts,
    /// How long a claim or a heartbeat holds a job, in seconds.
    pub lease: i64,
}

/// The source control the application ships: the landing of checks queued after each landing.
pub type ShippedSourceControl = SourceControlService<Arc<PostLandChecks>>;

impl Assembly {
    /// The authentication port.
    pub fn authentication(&self) -> AuthenticationService {
        AuthenticationService::new(Arc::clone(&self.identity))
    }

    /// The repositories port.
    pub fn repositories(&self) -> RepositoriesService {
        RepositoriesService::new(
            RepositoriesAdapters {
                directory: Arc::clone(&self.directory),
                authorisation: Arc::clone(&self.authorisation),
                membership: Arc::clone(&self.membership),
                changes: Arc::clone(&self.changes),
                delivery: Arc::clone(&self.delivery),
            },
            self.administration,
        )
    }

    /// The members port.
    pub fn members(&self) -> MembersService {
        MembersService::new(
            MembersAdapters {
                directory: Arc::clone(&self.directory),
                authorisation: Arc::clone(&self.authorisation),
                membership: Arc::clone(&self.membership),
            },
            self.administration,
        )
    }

    /// The registry token port of the registry `service`, signing through `issuance` and reading
    /// the time in seconds since 1970-01-01T00:00:00Z from `now`.
    pub fn registry_token(
        &self,
        issuance: Arc<dyn RegistryTokenIssuance>,
        service: String,
        now: fn() -> i64,
    ) -> RegistryTokenService {
        RegistryTokenService::new(
            RegistryTokenAdapters {
                directory: Arc::clone(&self.directory),
                authorisation: Arc::clone(&self.authorisation),
                issuance,
            },
            service,
            now,
        )
    }

    /// The source-control port, which queues the checks of each landing.
    pub fn source_control(&self) -> ShippedSourceControl {
        SourceControlService::new(
            SourceControlAdapters {
                objects: Arc::clone(&self.objects),
                log: Arc::clone(&self.log),
                derived: Arc::clone(&self.derived),
                drafts: Arc::clone(&self.drafts),
                cache_refs: Arc::clone(&self.cache_refs),
                directory: Arc::clone(&self.directory),
                authorisation: Arc::clone(&self.authorisation),
            },
            self.max_upload_size,
        )
        .with_landing_attempts(self.attempts)
        .with_after_landing(self.post_land_checks())
    }

    /// The post-land checks, the one place they are built, which source control, the jobs and the
    /// checks are each handed.
    fn post_land_checks(&self) -> Arc<PostLandChecks> {
        Arc::new(PostLandChecks::new(
            PostLandChecksAdapters {
                objects: Arc::clone(&self.objects),
                derived: Arc::clone(&self.derived),
                log: Arc::clone(&self.log),
                directory: Arc::clone(&self.directory),
                queue: Arc::clone(&self.queue),
                definitions: Arc::clone(&self.definitions),
            },
            self.attempts,
        ))
    }

    /// The runners port.
    pub fn runners(&self) -> RunnersService {
        RunnersService::new(RunnersAdapters {
            registry: Arc::clone(&self.registry),
            directory: Arc::clone(&self.directory),
            authorisation: Arc::clone(&self.authorisation),
        })
    }

    /// The jobs port.
    pub fn jobs(&self) -> JobsService {
        JobsService::new(
            JobsAdapters {
                registry: Arc::clone(&self.registry),
                queue: Arc::clone(&self.queue),
                cache_refs: Arc::clone(&self.cache_refs),
                objects: Arc::clone(&self.objects),
                derived: Arc::clone(&self.derived),
                log: Arc::clone(&self.log),
                drafts: Arc::clone(&self.drafts),
                authorisation: Arc::clone(&self.authorisation),
                directory: Arc::clone(&self.directory),
            },
            self.post_land_checks(),
            self.lease,
        )
    }

    /// The checks port.
    pub fn checks(&self) -> ChecksService {
        ChecksService::new(
            ChecksAdapters {
                queue: Arc::clone(&self.queue),
                log: Arc::clone(&self.log),
                directory: Arc::clone(&self.directory),
                authorisation: Arc::clone(&self.authorisation),
                objects: Arc::clone(&self.objects),
                derived: Arc::clone(&self.derived),
                definitions: Arc::clone(&self.definitions),
                drafts: Arc::clone(&self.drafts),
            },
            self.post_land_checks(),
        )
    }

    /// The runs port.
    pub fn runs(&self) -> RunsService {
        RunsService::new(RunsAdapters {
            queue: Arc::clone(&self.queue),
            directory: Arc::clone(&self.directory),
            authorisation: Arc::clone(&self.authorisation),
            drafts: Arc::clone(&self.drafts),
            derived: Arc::clone(&self.derived),
            objects: Arc::clone(&self.objects),
            log: Arc::clone(&self.log),
            cache_refs: Arc::clone(&self.cache_refs),
            definitions: Arc::clone(&self.definitions),
        })
    }
}
