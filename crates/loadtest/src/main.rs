//! A load harness for codehub: many concurrent agents drive two `codehub` replicas over HTTP
//! against the compose services, and the figures of each scenario are printed. It exits non-zero
//! where a replica dies or an operation answers a status the scenario does not expect.

mod api;
mod host;
mod proxy;
mod scenario;
mod services;
mod stats;

use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, ValueEnum};
use serde::Serialize;

use crate::host::Host;
use crate::scenario::{Limit, Plan, Scenario};
use crate::stats::Report;

/// Why the harness could not finish.
#[derive(Debug)]
pub struct Failure(String);

impl Failure {
    /// A failure with `message`.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Failure {}

macro_rules! failure_from {
    ($($error:ty),*) => {$(
        impl From<$error> for Failure {
            fn from(error: $error) -> Self {
                Self(error.to_string())
            }
        }
    )*};
}

failure_from!(
    std::io::Error,
    reqwest::Error,
    sqlx::Error,
    serde_json::Error
);

/// Which scenarios to run.
#[derive(Clone, Copy, Debug, ValueEnum)]
enum Choice {
    All,
    Disjoint,
    SameFile,
    ReadHeavy,
    External,
}

impl Choice {
    fn scenarios(self) -> Vec<Scenario> {
        match self {
            Self::All => Scenario::ALL.to_vec(),
            Self::Disjoint => vec![Scenario::Disjoint],
            Self::SameFile => vec![Scenario::SameFile],
            Self::ReadHeavy => vec![Scenario::ReadHeavy],
            Self::External => vec![Scenario::External],
        }
    }
}

#[derive(Parser)]
#[command(about = "Drives codehub with many concurrent agents and reports the figures")]
struct Cli {
    /// The scenario to run.
    #[arg(long, value_enum, default_value_t = Choice::All)]
    scenario: Choice,
    /// Concurrent agents, each with a draft of its own.
    #[arg(long, default_value_t = 200, value_parser = clap::value_parser!(u32).range(1..))]
    agents: u32,
    /// Seconds each scenario runs; 10 where neither this nor `--operations` is given.
    #[arg(long, conflicts_with = "operations", value_parser = clap::value_parser!(u64).range(1..))]
    duration: Option<u64>,
    /// Operations each agent makes, instead of a duration.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    operations: Option<u64>,
    /// Files each landing of `external` stages.
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..))]
    paths_per_landing: u32,
    /// Also write the figures, the commit and the machine to this file as JSON.
    #[arg(long)]
    json: Option<PathBuf>,
}

#[derive(Serialize)]
struct Parameters {
    agents: u32,
    replicas: u32,
    duration_seconds: Option<u64>,
    operations_per_agent: Option<u64>,
    paths_per_landing: u32,
}

#[derive(Serialize)]
struct Output<'a> {
    host: &'a Host,
    parameters: Parameters,
    scenarios: &'a [Report],
}

async fn run(cli: &Cli) -> Result<bool, Failure> {
    let binary = std::env::current_exe()?.with_file_name("codehub");
    if !binary.exists() {
        return Err(Failure::new(format!(
            "{} is not built; run `cargo build -p codehub` first",
            binary.display()
        )));
    }
    let (limit, duration_seconds, operations_per_agent) = match (cli.duration, cli.operations) {
        (_, Some(count)) => (Limit::Operations(count), None, Some(count)),
        (Some(seconds), None) => (
            Limit::Duration(Duration::from_secs(seconds)),
            Some(seconds),
            None,
        ),
        (None, None) => (Limit::Duration(Duration::from_secs(10)), Some(10), None),
    };
    let plan = Plan {
        agents: cli.agents as usize,
        limit,
        paths_per_landing: cli.paths_per_landing as usize,
    };
    let host = Host::read();
    println!("{}", host.line());
    let mut reports = Vec::new();
    for scenario in cli.scenario.scenarios() {
        let report = scenario::run(scenario, plan, &binary).await?;
        println!("{}", report.table());
        reports.push(report);
    }
    if let Some(path) = &cli.json {
        let output = Output {
            host: &host,
            parameters: Parameters {
                agents: cli.agents,
                replicas: 2,
                duration_seconds,
                operations_per_agent,
                paths_per_landing: cli.paths_per_landing,
            },
            scenarios: &reports,
        };
        std::fs::write(path, serde_json::to_string_pretty(&output)?)?;
    }
    Ok(reports.iter().all(Report::clean))
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(&Cli::parse()).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => {
            eprintln!("loadtest: an operation answered a status the scenario does not expect");
            ExitCode::FAILURE
        }
        Err(failure) => {
            eprintln!("loadtest: {failure}");
            ExitCode::FAILURE
        }
    }
}
