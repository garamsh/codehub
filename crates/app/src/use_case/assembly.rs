//! The one assembly of the driving ports from the driven ports, which the composition site and
//! every harness that serves the application build the shipped graph through.

use crate::use_case::administration::Administration;
use crate::use_case::authentication::AuthenticationService;
use crate::use_case::checked_source_control::{CheckedAdapters, CheckedSourceControl};
use crate::use_case::checks::{ChecksAdapters, ChecksService};
use crate::use_case::jobs::{JobsAdapters, JobsService};
use crate::use_case::landing_attempts::LandingAttempts;
use crate::use_case::members::{MembersAdapters, MembersService};
use crate::use_case::repositories::{RepositoriesAdapters, RepositoriesService};
use crate::use_case::runners::{RunnersAdapters, RunnersService};
use crate::use_case::source_control::{SourceControlAdapters, SourceControlService};

/// The driving ports as the application ships them, over the driven ports it is given: source
/// control queues the checks of each landing, and the jobs and the checks revert a changeset
/// whose blocking check failed. Its fields are what the driving ports are built over.
pub struct Assembly<'a, O, L, D, S, R, A, M, Q, G, C, X, V, I> {
    /// Stores blobs.
    pub objects: &'a O,
    /// Stores changesets and the refs.
    pub log: &'a L,
    /// Stores derived data.
    pub derived: &'a D,
    /// Stores drafts.
    pub drafts: &'a S,
    /// Keeps organisations and repositories.
    pub directory: &'a R,
    /// Says what callers may do.
    pub authorisation: &'a A,
    /// Keeps members and their grants.
    pub membership: &'a M,
    /// Holds the jobs.
    pub queue: &'a Q,
    /// Finds the callers' registrations.
    pub registry: &'a G,
    /// Reads the check definitions.
    pub definitions: &'a C,
    /// Receives each organisation creation.
    pub changes: &'a X,
    /// Delivers the entries of a repository's creation once it has committed.
    pub delivery: &'a V,
    /// Verifies credentials.
    pub identity: &'a I,
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
pub type ShippedSourceControl<'a, O, L, D, S, R, A, Q, C> =
    CheckedSourceControl<'a, SourceControlService<'a, O, L, D, S, R, A>, O, D, L, R, Q, C>;

impl<'a, O, L, D, S, R, A, M, Q, G, C, X, V, I>
    Assembly<'a, O, L, D, S, R, A, M, Q, G, C, X, V, I>
{
    /// The authentication port.
    pub const fn authentication(&self) -> AuthenticationService<'a, I> {
        AuthenticationService::new(self.identity)
    }

    /// The repositories port.
    pub const fn repositories(&self) -> RepositoriesService<'a, R, A, M, X, V> {
        RepositoriesService::new(
            RepositoriesAdapters {
                directory: self.directory,
                authorisation: self.authorisation,
                membership: self.membership,
                changes: self.changes,
                delivery: self.delivery,
            },
            self.administration,
        )
    }

    /// The members port.
    pub const fn members(&self) -> MembersService<'a, R, A, M> {
        MembersService::new(
            MembersAdapters {
                directory: self.directory,
                authorisation: self.authorisation,
                membership: self.membership,
            },
            self.administration,
        )
    }

    /// The source-control port, which queues the checks of each landing.
    pub fn source_control(&self) -> ShippedSourceControl<'a, O, L, D, S, R, A, Q, C> {
        CheckedSourceControl::new(
            SourceControlService::new(
                SourceControlAdapters {
                    objects: self.objects,
                    log: self.log,
                    derived: self.derived,
                    drafts: self.drafts,
                    directory: self.directory,
                    authorisation: self.authorisation,
                },
                self.max_upload_size,
            )
            .with_landing_attempts(self.attempts),
            CheckedAdapters {
                objects: self.objects,
                derived: self.derived,
                log: self.log,
                directory: self.directory,
                queue: self.queue,
                definitions: self.definitions,
            },
            self.attempts,
        )
    }

    /// The runners port.
    pub const fn runners(&self) -> RunnersService<'a, G, R, A> {
        RunnersService::new(RunnersAdapters {
            registry: self.registry,
            directory: self.directory,
            authorisation: self.authorisation,
        })
    }

    /// The jobs port.
    pub const fn jobs(&self) -> JobsService<'a, G, Q, O, D, L, R, C> {
        JobsService::new(
            JobsAdapters {
                registry: self.registry,
                queue: self.queue,
                objects: self.objects,
                derived: self.derived,
                log: self.log,
                directory: self.directory,
                definitions: self.definitions,
            },
            self.lease,
            self.attempts,
        )
    }

    /// The checks port.
    pub const fn checks(&self) -> ChecksService<'a, Q, L, R, A, O, D, C> {
        ChecksService::new(
            ChecksAdapters {
                queue: self.queue,
                log: self.log,
                directory: self.directory,
                authorisation: self.authorisation,
                objects: self.objects,
                derived: self.derived,
                definitions: self.definitions,
            },
            self.attempts,
        )
    }
}
