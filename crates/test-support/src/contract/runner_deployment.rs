//! The deployment the runner and check scenarios run against, and the scenarios' own vocabulary.

use std::collections::BTreeSet;

use app::domain::address::Address;
use app::domain::check::{CheckDefinition, Command, Need, Pattern, Timeout};
use app::domain::name::{Label, OrganisationName};
use app::domain::principal::Caller;
use app::domain::run::RunSettings;
use app::port::driven::check_definitions::{CheckDefinitions, CheckFileError};
use app::port::driving::runners::Runners;
use app::port::driving::source_control::{SourceControl, SourceControlError};
use app::use_case::assembly::ShippedSourceControl;
use app::use_case::checks::ChecksService;
use app::use_case::jobs::JobsService;
use app::use_case::runners::RunnersService;

use super::{Deployment, Repo, draft};

/// How long a claim or a heartbeat holds a job, in seconds.
pub const LEASE: i64 = 30;

/// A deployment whose check definitions the scenarios can make read a file as a given outcome,
/// with the driving ports of the runners and the checks over its adapters.
pub trait RunnerDeployment: Deployment {
    /// Makes the check definitions read the file content `text` as `outcome`. A deployment whose
    /// definitions parse the file format itself ignores `outcome`, so `text` is always in that
    /// format and `outcome` is what it reads as.
    fn teach(&self, text: &str, outcome: Result<Vec<CheckDefinition>, CheckFileError>);

    /// Makes the run settings file read the content `text` as `outcome`, as
    /// [`RunnerDeployment::teach`] does for a check file.
    fn teach_run(&self, text: &str, outcome: Result<RunSettings, CheckFileError>);

    /// The runners port over this deployment's adapters.
    fn runners(&self) -> RunnersService {
        self.assembly().runners()
    }

    /// The jobs port over this deployment's adapters, with the lease [`LEASE`].
    fn jobs(&self) -> JobsService {
        self.assembly().jobs()
    }

    /// The checks port over this deployment's adapters.
    fn checks(&self) -> ChecksService {
        self.assembly().checks()
    }

    /// Source control that queues the checks of what it lands.
    fn checked_source_control(&self) -> ShippedSourceControl {
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
    /// The checks it needs, as a check file writes them.
    pub needs: &'static [&'static str],
    /// The input patterns it reads, as a check file writes them; none reads the whole folder.
    pub inputs: &'static [&'static str],
    /// The paths, relative to its folder, it produces.
    pub outputs: &'static [&'static str],
    /// The paths, relative to its folder, it uses as a cache.
    pub caches: &'static [&'static str],
}

/// A blocking check named `name` that requires no label and has the default timeout.
#[must_use]
pub const fn check(name: &'static str) -> Spec {
    Spec {
        name,
        labels: &[],
        blocking: true,
        timeout: 600,
        needs: &[],
        inputs: &[],
        outputs: &[],
        caches: &[],
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

    /// The same check, needing `needs`, as a check file writes them.
    #[must_use]
    pub const fn needing(self, needs: &'static [&'static str]) -> Self {
        Self { needs, ..self }
    }

    /// The same check reading `inputs`, as a check file writes them.
    #[must_use]
    pub const fn reading(self, inputs: &'static [&'static str]) -> Self {
        Self { inputs, ..self }
    }

    /// The same check producing `outputs`, relative to its folder.
    #[must_use]
    pub const fn producing(self, outputs: &'static [&'static str]) -> Self {
        Self { outputs, ..self }
    }

    /// The same check caching `caches`, relative to its folder.
    #[must_use]
    pub const fn caching(self, caches: &'static [&'static str]) -> Self {
        Self { caches, ..self }
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
        let list = |items: &[&str]| -> String {
            let items: Vec<String> = items.iter().map(|item| format!("{item:?}")).collect();
            items.join(", ")
        };
        text.push_str(&format!(
            "[[check]]\nname = {:?}\ncommand = \"run {}\"\nlabels = [{}]\nblocking = {}\ntimeout = {}\nneeds = [{}]\noutputs = [{}]\ncaches = [{}]\n",
            spec.name,
            spec.name,
            labels.join(", "),
            spec.blocking,
            spec.timeout,
            list(spec.needs),
            list(spec.outputs),
            list(spec.caches),
        ));
        if !spec.inputs.is_empty() {
            text.push_str(&format!("inputs = [{}]\n", list(spec.inputs)));
        }
        text.push('\n');
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
        .map(|spec| {
            let mut definition = CheckDefinition::new(
                spec.name.parse().unwrap(),
                Command::new(&format!("run {}", spec.name)).unwrap(),
                labels(spec.labels),
                spec.blocking,
                Timeout::new(spec.timeout).unwrap(),
            );
            definition.needs = spec
                .needs
                .iter()
                .map(|need| need.parse::<Need>().unwrap())
                .collect();
            if !spec.inputs.is_empty() {
                definition.inputs = spec
                    .inputs
                    .iter()
                    .map(|input| input.parse::<Pattern>().unwrap())
                    .collect();
            }
            definition.outputs = spec
                .outputs
                .iter()
                .map(|path| path.parse().unwrap())
                .collect();
            definition.caches = spec
                .caches
                .iter()
                .map(|path| path.parse().unwrap())
                .collect();
            definition
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

/// Lands a changeset that defines, in each folder of `folders`, the checks `specs` (the root
/// folder is `""`), teaching the definitions what each file reads as, and returns it.
///
/// # Panics
///
/// Where the landing fails.
pub async fn land_checks_in<H: RunnerDeployment>(
    repo: &Repo<'_, H>,
    caller: &Caller,
    name: &str,
    folders: &[(&str, &[Spec])],
) -> Address {
    land_checks_with(repo, caller, name, folders, &[]).await
}

/// Lands a changeset that defines, in each folder of `folders`, the checks `specs` and stages
/// `other` files beside them, and returns it.
///
/// # Panics
///
/// Where the landing fails.
pub async fn land_checks_with<H: RunnerDeployment>(
    repo: &Repo<'_, H>,
    caller: &Caller,
    name: &str,
    folders: &[(&str, &[Spec])],
    other: &[(&str, &str)],
) -> Address {
    let file = repo.harness.definitions().file().to_string();
    let mut files = Vec::new();
    for (folder, specs) in folders {
        let text = text(specs);
        repo.harness.teach(&text, Ok(definitions(specs)));
        let path = if folder.is_empty() {
            file.clone()
        } else {
            format!("{folder}/{file}")
        };
        files.push((path, text));
    }
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .chain(other.iter().copied())
        .collect();
    land(repo, caller, name, &files).await.unwrap()
}
