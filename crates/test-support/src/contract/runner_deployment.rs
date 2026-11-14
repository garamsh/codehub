//! The deployment of the runner registry, the job queue and the check definitions, which the
//! runner and check scenarios run against.

use std::collections::BTreeSet;

use adapter_memory::{MemoryIdentityVerification, MemoryObjectStore};
use app::domain::address::Address;
use app::domain::check::{CheckDefinition, Command, Timeout};
use app::domain::name::{Label, OrganisationName};
use app::domain::principal::Caller;
use app::port::driven::check_definitions::{CheckDefinitions, CheckFileError};
use app::port::driven::job_queue::JobQueue;
use app::port::driven::runner_registry::RunnerRegistry;
use app::port::driving::runners::Runners;
use app::port::driving::source_control::{SourceControl, SourceControlError};
use app::use_case::administration::Administration;
use app::use_case::assembly::{Assembly, ShippedSourceControl};
use app::use_case::checks::ChecksService;
use app::use_case::jobs::JobsService;
use app::use_case::landing_attempts::LandingAttempts;
use app::use_case::runners::RunnersService;

use super::{Deployment, MAX_UPLOAD_SIZE, Repo, draft};

/// Source control that queues the checks of what it lands, over a deployment's adapters.
pub type Checked<'a, H> = ShippedSourceControl<
    'a,
    MemoryObjectStore,
    <H as Deployment>::Log,
    <H as Deployment>::Derived,
    <H as Deployment>::Drafts,
    <H as Deployment>::Directory,
    <H as Deployment>::Authorisation,
    <H as RunnerDeployment>::Queue,
    <H as RunnerDeployment>::Definitions,
>;

/// The jobs service over a deployment's adapters.
pub type JobsOf<'a, H> = JobsService<
    'a,
    <H as RunnerDeployment>::Registry,
    <H as RunnerDeployment>::Queue,
    MemoryObjectStore,
    <H as Deployment>::Derived,
    <H as Deployment>::Log,
    <H as Deployment>::Directory,
    <H as RunnerDeployment>::Definitions,
>;

/// The checks service over a deployment's adapters.
pub type ChecksOf<'a, H> = ChecksService<
    'a,
    <H as RunnerDeployment>::Queue,
    <H as Deployment>::Log,
    <H as Deployment>::Directory,
    <H as Deployment>::Authorisation,
    MemoryObjectStore,
    <H as Deployment>::Derived,
    <H as RunnerDeployment>::Definitions,
>;

/// The driving ports over a deployment's adapters.
pub type AssemblyOf<'a, H> = Assembly<
    'a,
    MemoryObjectStore,
    <H as Deployment>::Log,
    <H as Deployment>::Derived,
    <H as Deployment>::Drafts,
    <H as Deployment>::Directory,
    <H as Deployment>::Authorisation,
    <H as Deployment>::Membership,
    <H as RunnerDeployment>::Queue,
    <H as RunnerDeployment>::Registry,
    <H as RunnerDeployment>::Definitions,
    <H as Deployment>::Changes,
    <H as Deployment>::Delivery,
    MemoryIdentityVerification,
>;

/// How long a claim or a heartbeat holds a job, in seconds.
pub const LEASE: i64 = 30;

/// The adapters of the runner registry, the job queue and the check definitions of one
/// deployment, over the adapters of its [`Deployment`].
pub trait RunnerDeployment: Deployment {
    /// The runner registry.
    type Registry: RunnerRegistry + Sync;
    /// The job queue.
    type Queue: JobQueue + Sync;
    /// The check definitions.
    type Definitions: CheckDefinitions + Sync;

    /// The runner registry.
    fn registry(&self) -> &Self::Registry;
    /// The job queue.
    fn queue(&self) -> &Self::Queue;
    /// The check definitions.
    fn definitions(&self) -> &Self::Definitions;

    /// Makes the check definitions read the file content `text` as `outcome`. A deployment whose
    /// definitions parse the file format itself ignores `outcome`, so `text` is always in that
    /// format and `outcome` is what it reads as.
    fn teach(&self, text: &str, outcome: Result<Vec<CheckDefinition>, CheckFileError>);

    /// The driving ports over this deployment's adapters, assembled as the application ships
    /// them, with the lease [`LEASE`].
    fn assembly(&self) -> AssemblyOf<'_, Self> {
        Assembly {
            objects: self.objects(),
            log: self.log(),
            derived: self.derived(),
            drafts: self.drafts(),
            directory: self.directory(),
            authorisation: self.authorisation(),
            membership: self.membership(),
            queue: self.queue(),
            registry: self.registry(),
            definitions: self.definitions(),
            changes: self.changes(),
            delivery: self.delivery(),
            identity: self.identity(),

            administration: Administration::BuiltIn,
            max_upload_size: MAX_UPLOAD_SIZE,
            attempts: LandingAttempts::default(),
            lease: LEASE,
        }
    }

    /// The runners port over this deployment's adapters.
    fn runners(&self) -> RunnersService<'_, Self::Registry, Self::Directory, Self::Authorisation> {
        self.assembly().runners()
    }

    /// The jobs port over this deployment's adapters, with the lease [`LEASE`].
    fn jobs(&self) -> JobsOf<'_, Self> {
        self.assembly().jobs()
    }

    /// The checks port over this deployment's adapters.
    fn checks(&self) -> ChecksOf<'_, Self> {
        self.assembly().checks()
    }

    /// Source control that queues the checks of what it lands.
    fn checked_source_control(&self) -> Checked<'_, Self> {
        self.assembly().source_control()
    }
}

/// One check as a scenario writes it.
#[derive(Clone, Copy)]
pub struct Spec {
    /// The check's name.
    pub name: &'static str,
    /// The labels it requires.
    pub labels: &'static [&'static str],
    /// Whether `last-green` waits for it.
    pub blocking: bool,
    /// Its timeout in seconds.
    pub timeout: u64,
}

/// A blocking check named `name` that requires no label and has the default timeout.
#[must_use]
pub const fn check(name: &'static str) -> Spec {
    Spec {
        name,
        labels: &[],
        blocking: true,
        timeout: 600,
    }
}

impl Spec {
    /// The same check, requiring `labels`.
    #[must_use]
    pub const fn labelled(self, labels: &'static [&'static str]) -> Self {
        Self { labels, ..self }
    }

    /// The same check, not blocking.
    #[must_use]
    pub const fn non_blocking(self) -> Self {
        Self {
            blocking: false,
            ..self
        }
    }

    /// The same check with a timeout of `timeout` seconds.
    #[must_use]
    pub const fn timing_out_after(self, timeout: u64) -> Self {
        Self { timeout, ..self }
    }
}

/// The text of a check definitions file listing `specs`, in TOML.
#[must_use]
pub fn text(specs: &[Spec]) -> String {
    let mut text = String::new();
    for spec in specs {
        let labels: Vec<String> = spec
            .labels
            .iter()
            .map(|label| format!("{label:?}"))
            .collect();
        text.push_str(&format!(
            "[[check]]\nname = {:?}\ncommand = \"run {}\"\nlabels = [{}]\nblocking = {}\ntimeout = {}\n\n",
            spec.name,
            spec.name,
            labels.join(", "),
            spec.blocking,
            spec.timeout
        ));
    }
    text
}

/// The definitions `specs` read as.
///
/// # Panics
///
/// Where a spec is not a valid check.
#[must_use]
pub fn definitions(specs: &[Spec]) -> Vec<CheckDefinition> {
    specs
        .iter()
        .map(|spec| CheckDefinition {
            name: spec.name.parse().unwrap(),
            command: Command::new(&format!("run {}", spec.name)).unwrap(),
            labels: labels(spec.labels),
            blocking: spec.blocking,
            timeout: Timeout::new(spec.timeout).unwrap(),
        })
        .collect()
}

/// The labels `names`.
///
/// # Panics
///
/// Where a name is not a label.
#[must_use]
pub fn labels(names: &[&str]) -> BTreeSet<Label> {
    names.iter().map(|name| name.parse().unwrap()).collect()
}

/// The organisation `name`.
///
/// # Panics
///
/// Where `name` is not a name.
#[must_use]
pub fn organisation(name: &str) -> OrganisationName {
    name.parse().unwrap()
}

/// Registers the principal `name` as a runner of `organisation` for every repository, offering
/// `offered`, as `admin`, and returns the runner as a caller.
///
/// # Panics
///
/// Where registering fails.
pub async fn runner<H: RunnerDeployment>(
    harness: &H,
    admin: &Caller,
    organisation: &OrganisationName,
    name: &str,
    offered: &[&str],
) -> Caller {
    harness
        .runners()
        .register(
            admin,
            organisation,
            None,
            &H::principal(name),
            labels(offered),
        )
        .await
        .unwrap();
    harness.caller(name).await
}

/// Stages `files` in `caller`'s draft `name` and lands it through source control that queues
/// checks.
///
/// # Errors
///
/// The error the landing fails with.
///
/// # Panics
///
/// Where staging fails.
pub async fn land<H: RunnerDeployment>(
    repo: &Repo<'_, H>,
    caller: &Caller,
    name: &str,
    files: &[(&str, &str)],
) -> Result<Address, SourceControlError> {
    let draft = draft(name);
    repo.stage(caller, &draft, files).await;
    let landing = app::port::driving::source_control::Landing {
        message: name.to_owned(),
        timestamp: 0,
    };
    repo.harness
        .checked_source_control()
        .land(caller, repo.id, &draft, landing)
        .await
}

/// Lands a changeset that defines `specs` as the repository's checks, teaching the definitions
/// what its file reads as, and returns it.
///
/// # Panics
///
/// Where the landing fails.
pub async fn land_checks<H: RunnerDeployment>(
    repo: &Repo<'_, H>,
    caller: &Caller,
    name: &str,
    specs: &[Spec],
) -> Address {
    let text = text(specs);
    repo.harness.teach(&text, Ok(definitions(specs)));
    let file = repo.harness.definitions().file().to_string();
    land(repo, caller, name, &[(&file, &text)]).await.unwrap()
}
