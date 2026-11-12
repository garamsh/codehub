//! The four scenarios: what each sets up, and what each agent does until the run is over.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::ValueEnum;
use reqwest::Method;
use serde_json::json;
use tokio::task::JoinSet;

use crate::Failure;
use crate::api::{Api, Session};
use crate::proxy::Proxy;
use crate::services::{self, Bucket, Database, Replica};
use crate::stats::{ExternalCalls, Kind, Recorder, Report, succeeded};

/// How many `codehub` processes a scenario starts; agents are spread over them round robin.
const REPLICAS: usize = 2;

/// The organisation and repository every agent works in.
const ORGANISATION: &str = "acme";
const REPOSITORY: &str = "app";

/// How many times a landing is attempted before it is given up on.
const ATTEMPTS: u64 = 50;

/// Lines of the shared file each agent of `same-file` owns.
const OWNED_LINES: usize = 3;

/// Files `read-heavy` seeds and reads.
const SEEDED_FILES: u64 = 20;

/// How often the replicas are checked for being alive.
const WATCH: Duration = Duration::from_millis(250);

/// A kind of load.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Scenario {
    /// Each agent lands a file of its own.
    Disjoint,
    /// Each agent edits its own lines of one shared file.
    SameFile,
    /// About nine operations in ten read a file or its history, the rest land.
    ReadHeavy,
    /// Introspection identity and AuthZEN authorisation, many paths per landing.
    External,
}

impl Scenario {
    /// Every scenario, in the order `all` runs them.
    pub const ALL: [Self; 4] = [
        Self::Disjoint,
        Self::SameFile,
        Self::ReadHeavy,
        Self::External,
    ];

    /// The name the table prints and the command line takes.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Disjoint => "disjoint",
            Self::SameFile => "same-file",
            Self::ReadHeavy => "read-heavy",
            Self::External => "external",
        }
    }
}

/// When an agent stops.
#[derive(Clone, Copy)]
pub enum Limit {
    /// After this long.
    Duration(Duration),
    /// After this many operations.
    Operations(u64),
}

/// The parameters of a run.
#[derive(Clone, Copy)]
pub struct Plan {
    /// Concurrent agents.
    pub agents: usize,
    /// When each stops.
    pub limit: Limit,
    /// Files per landing of `external`.
    pub paths_per_landing: usize,
}

/// What a landing stages.
#[derive(Clone, Copy)]
enum Work {
    /// One file of the agent's own.
    Own,
    /// The agent's lines of the shared file.
    Shared,
    /// Many files under `docs/`.
    Many,
}

struct Context {
    api: Api,
    scenario: Scenario,
    plan: Plan,
    bases: Vec<String>,
    tokens: Vec<String>,
    deadline: Option<Instant>,
}

impl Context {
    fn session(&self, agent: usize) -> Session<'_> {
        self.api.session(
            pick(&self.bases, agent),
            pick(&self.tokens, agent),
            ORGANISATION,
            REPOSITORY,
        )
    }

    fn finished(&self, operations: u64) -> bool {
        match self.plan.limit {
            Limit::Duration(_) => self.deadline.is_some_and(|end| Instant::now() >= end),
            Limit::Operations(count) => operations >= count,
        }
    }
}

/// Item `index` of `items`, wrapping round; empty where there are none.
fn pick(items: &[String], index: usize) -> &str {
    items
        .get(index % items.len().max(1))
        .map_or("", String::as_str)
}

/// Runs `scenario` against a fresh database and bucket and two fresh replicas, and removes what it
/// created whatever the outcome.
///
/// # Errors
///
/// Fails where a service or replica cannot be set up, a replica dies, or the cleanup fails.
pub async fn run(scenario: Scenario, plan: Plan, binary: &Path) -> Result<Report, Failure> {
    let database = Database::create().await?;
    let bucket = match Bucket::create().await {
        Ok(bucket) => bucket,
        Err(error) => {
            let _ = database.remove().await;
            return Err(error);
        }
    };
    let outcome = measure(scenario, plan, binary, &database, &bucket).await;
    let bucket_removed = bucket.remove().await;
    let database_removed = database.remove().await;
    let report = outcome?;
    bucket_removed?;
    database_removed?;
    Ok(report)
}

/// The settings that make a replica verify Keycloak tokens itself and keep its own members.
fn oidc_settings() -> Result<Vec<(&'static str, String)>, Failure> {
    Ok(vec![
        (
            "CODEHUB_OIDC_ISSUER",
            services::env("CODEHUB_TEST_KEYCLOAK_ISSUER")?,
        ),
        (
            "CODEHUB_OIDC_AUDIENCE",
            services::env("CODEHUB_TEST_KEYCLOAK_AUDIENCE")?,
        ),
    ])
}

/// The settings of the external mode, with the introspection endpoint at `introspection` and the
/// decision point at `decisions`.
fn external_settings(
    introspection: &str,
    decisions: &str,
) -> Result<Vec<(&'static str, String)>, Failure> {
    let issuer = services::env("CODEHUB_TEST_KEYCLOAK_ISSUER")?;
    Ok(vec![
        ("CODEHUB_IDENTITY", "introspection".to_owned()),
        ("CODEHUB_INTROSPECTION_ENDPOINT", introspection.to_owned()),
        (
            "CODEHUB_INTROSPECTION_CLIENT_ID",
            services::env("CODEHUB_TEST_KEYCLOAK_INTROSPECTION_CLIENT_ID")?,
        ),
        (
            "CODEHUB_INTROSPECTION_CLIENT_SECRET",
            services::env("CODEHUB_TEST_KEYCLOAK_INTROSPECTION_CLIENT_SECRET")?,
        ),
        ("CODEHUB_INTROSPECTION_ISSUER", issuer.clone()),
        (
            "CODEHUB_INTROSPECTION_AUDIENCE",
            services::env("CODEHUB_TEST_KEYCLOAK_AUDIENCE")?,
        ),
        ("CODEHUB_AUTHORISATION", "authzen".to_owned()),
        ("CODEHUB_AUTHZEN_URL", decisions.to_owned()),
        (
            "CODEHUB_AUTHZEN_TOKEN_ENDPOINT",
            format!("{issuer}/protocol/openid-connect/token"),
        ),
        (
            "CODEHUB_AUTHZEN_CLIENT_ID",
            services::env("CODEHUB_TEST_KEYCLOAK_AUTHZEN_CLIENT_ID")?,
        ),
        (
            "CODEHUB_AUTHZEN_CLIENT_SECRET",
            services::env("CODEHUB_TEST_KEYCLOAK_AUTHZEN_CLIENT_SECRET")?,
        ),
    ])
}

/// The origin of `url` and its path: `http://host:port` and `/path`.
fn split_origin(url: &str) -> Result<(String, String), Failure> {
    let parsed: reqwest::Url = url
        .parse()
        .map_err(|_| Failure::new(format!("{url} is not a URL")))?;
    let origin = parsed.origin().ascii_serialization();
    Ok((origin, parsed.path().to_owned()))
}

/// A status that is not a success fails the setup.
fn check(step: &str, status: u16) -> Result<(), Failure> {
    if succeeded(status) {
        Ok(())
    } else {
        Err(Failure::new(format!("setup: {step} answered {status}")))
    }
}

async fn measure(
    scenario: Scenario,
    plan: Plan,
    binary: &Path,
    database: &Database,
    bucket: &Bucket,
) -> Result<Report, Failure> {
    let external = scenario == Scenario::External;
    let (settings, proxies) = if external {
        let keycloak = services::env("CODEHUB_TEST_KEYCLOAK_ISSUER")?;
        let (keycloak_origin, issuer_path) = split_origin(&keycloak)?;
        let (cerbos_origin, _) = split_origin(&services::env("CODEHUB_TEST_AUTHZEN_URL")?)?;
        let introspection = Proxy::start(&keycloak_origin).await?;
        let decisions = Proxy::start(&cerbos_origin).await?;
        let endpoint = format!(
            "{}{issuer_path}/protocol/openid-connect/token/introspect",
            introspection.origin
        );
        (
            external_settings(&endpoint, &decisions.origin)?,
            Some((introspection, decisions)),
        )
    } else {
        (oidc_settings()?, None)
    };

    let mut replicas = Vec::new();
    for _ in 0..REPLICAS {
        replicas.push(Replica::start(binary, database, bucket, &settings).await?);
    }
    let bases: Vec<String> = replicas
        .iter()
        .map(|replica| replica.base.clone())
        .collect();
    // Tokens last minutes; they are fetched now so the whole measured period is inside one.
    let alice = services::token("TEST", "alice").await?;
    let bob = services::token("TEST", "bob").await?;
    let mut context = Context {
        api: Api::new(),
        scenario,
        plan,
        bases,
        tokens: vec![alice, bob],
        deadline: None,
    };
    prepare(&context).await?;

    let before = proxies
        .as_ref()
        .map(|(introspection, decisions)| (introspection.count(), decisions.count()));
    let requests_before = context.api.requests();
    let started = Instant::now();
    if let Limit::Duration(length) = plan.limit {
        context.deadline = Some(started + length);
    }
    let context = Arc::new(context);
    let mut agents = JoinSet::new();
    for index in 0..plan.agents {
        agents.spawn(agent(context.clone(), index));
    }
    let collect = async {
        let mut total = Recorder::default();
        while let Some(joined) = agents.join_next().await {
            total.merge(joined.map_err(|error| Failure::new(format!("an agent failed: {error}")))?);
        }
        Ok::<_, Failure>(total)
    };
    let total = tokio::select! {
        total = collect => total?,
        failure = watch(&mut replicas) => return Err(failure),
    };
    let elapsed = started.elapsed();

    // A replica that died at the very end is not seen by the watch.
    for replica in &mut replicas {
        if let Some(status) = replica.exited()? {
            return Err(Failure::new(format!("a replica exited: {status}")));
        }
    }
    for replica in replicas {
        replica.stop().await;
    }

    let mut report = total.report(scenario.name(), plan.agents, elapsed);
    if let (Some((introspection, decisions)), Some((introspection_before, decisions_before))) =
        (proxies, before)
    {
        report.external_calls = Some(ExternalCalls::new(
            context.api.requests() - requests_before,
            decisions.count() - decisions_before,
            introspection.count() - introspection_before,
        ));
    }
    Ok(report)
}

/// Waits until a replica is no longer running, and says how it ended.
async fn watch(replicas: &mut [Replica]) -> Failure {
    loop {
        tokio::time::sleep(WATCH).await;
        for replica in replicas.iter_mut() {
            match replica.exited() {
                Ok(Some(status)) => {
                    return Failure::new(format!("a replica exited during the run: {status}"));
                }
                Ok(None) => {}
                Err(error) => return error,
            }
        }
    }
}

/// Creates the organisation and repository, and what the scenario starts from.
async fn prepare(context: &Context) -> Result<(), Failure> {
    let admin = context.session(0);
    if context.scenario == Scenario::External {
        // The decision point lets alice create `acme/app`, and the organisation with it.
        check(
            "creating the repository",
            admin
                .administer(
                    Method::POST,
                    &format!("orgs/{ORGANISATION}/repos"),
                    &json!({ "name": REPOSITORY }),
                )
                .await,
        )?;
    } else {
        check(
            "creating the organisation",
            admin
                .administer(Method::POST, "orgs", &json!({ "name": ORGANISATION }))
                .await,
        )?;
        check(
            "creating the repository",
            admin
                .administer(
                    Method::POST,
                    &format!("orgs/{ORGANISATION}/repos"),
                    &json!({ "name": REPOSITORY }),
                )
                .await,
        )?;
        let issuer = services::env("CODEHUB_TEST_KEYCLOAK_ISSUER")?;
        let bob = services::env("CODEHUB_TEST_KEYCLOAK_SUBJECT_BOB")?;
        check(
            "adding bob",
            admin
                .administer(
                    Method::PUT,
                    &format!("orgs/{ORGANISATION}/members"),
                    &json!({ "issuer": issuer, "subject": bob, "role": "member" }),
                )
                .await,
        )?;
        check(
            "granting bob",
            admin
                .administer(
                    Method::PUT,
                    &format!("orgs/{ORGANISATION}/repos/{REPOSITORY}/grants"),
                    &json!({ "issuer": issuer, "subject": bob, "prefix": "", "access": "write" }),
                )
                .await,
        )?;
    }
    // A replica fetches the issuer's keys on its first request, and requests that arrive while it
    // does are rejected; one request to each replica keeps that start-up out of the figures.
    for replica in 0..REPLICAS {
        context.session(replica).log("warm-up").await;
    }
    match context.scenario {
        Scenario::SameFile => {
            let text: String = (0..context.plan.agents * OWNED_LINES)
                .map(|line| format!("line {line}\n"))
                .collect();
            seed(&admin, &[("shared.txt".to_owned(), text)]).await
        }
        Scenario::ReadHeavy => {
            let files: Vec<(String, String)> = (0..SEEDED_FILES)
                .map(|number| (seeded(number), format!("seeded file {number}\n")))
                .collect();
            seed(&admin, &files).await
        }
        Scenario::Disjoint | Scenario::External => Ok(()),
    }
}

fn seeded(number: u64) -> String {
    format!("seed/file-{number}.txt")
}

/// Lands `files` in one landing.
async fn seed(session: &Session<'_>, files: &[(String, String)]) -> Result<(), Failure> {
    for (path, text) in files {
        check(
            "staging a seed file",
            session.stage("seed", path, text.as_bytes()).await,
        )?;
    }
    check("landing the seed", session.land("seed", "seed").await)
}

/// What one agent does until it is told to stop; stops early on a status it did not expect.
async fn agent(context: Arc<Context>, index: usize) -> Recorder {
    let mut recorder = Recorder::default();
    let session = context.session(index);
    let mut random = Random::new(index);
    let mut drafts = 0;
    let mut operations = 0;
    while !context.finished(operations) {
        let succeeded = match context.scenario {
            Scenario::Disjoint => {
                land(
                    &session,
                    &mut recorder,
                    index,
                    operations,
                    &mut drafts,
                    Work::Own,
                    &context,
                )
                .await
            }
            Scenario::SameFile => {
                land(
                    &session,
                    &mut recorder,
                    index,
                    operations,
                    &mut drafts,
                    Work::Shared,
                    &context,
                )
                .await
            }
            Scenario::External => {
                land(
                    &session,
                    &mut recorder,
                    index,
                    operations,
                    &mut drafts,
                    Work::Many,
                    &context,
                )
                .await
            }
            Scenario::ReadHeavy => {
                let roll = random.below(100);
                let path = seeded(random.below(SEEDED_FILES));
                if roll < 45 {
                    let started = Instant::now();
                    let (status, _) = session.read(&path).await;
                    recorder.record(Kind::Read, started.elapsed(), status);
                    succeeded(status)
                } else if roll < 90 {
                    let started = Instant::now();
                    let status = session.log(&path).await;
                    recorder.record(Kind::Log, started.elapsed(), status);
                    succeeded(status)
                } else {
                    land(
                        &session,
                        &mut recorder,
                        index,
                        operations,
                        &mut drafts,
                        Work::Own,
                        &context,
                    )
                    .await
                }
            }
        };
        recorder.operation();
        operations += 1;
        if !succeeded {
            break;
        }
    }
    recorder
}

/// Stages `work` in a draft and lands it; where the landing answers 409 the work is staged again in
/// a fresh draft, because a failed landing leaves its draft on the base it had. Every 409 is one
/// retry. Whether the landing succeeded.
async fn land(
    session: &Session<'_>,
    recorder: &mut Recorder,
    agent: usize,
    operation: u64,
    drafts: &mut u64,
    work: Work,
    context: &Context,
) -> bool {
    let mut retries = 0;
    loop {
        *drafts += 1;
        let draft = format!("agent-{agent}-{drafts}");
        if !stage(session, recorder, agent, operation, &draft, work, context).await {
            recorder.gave_up(retries);
            return false;
        }
        let started = Instant::now();
        let status = session.land(&draft, "load").await;
        recorder.record(Kind::Land, started.elapsed(), status);
        if succeeded(status) {
            recorder.landed(retries);
            return true;
        }
        if status != 409 {
            recorder.gave_up(retries);
            return false;
        }
        retries += 1;
        if retries >= ATTEMPTS {
            recorder.gave_up(retries);
            recorder.abandoned();
            return false;
        }
    }
}

/// Stages one attempt of `work` in `draft`; whether every call succeeded.
async fn stage(
    session: &Session<'_>,
    recorder: &mut Recorder,
    agent: usize,
    operation: u64,
    draft: &str,
    work: Work,
    context: &Context,
) -> bool {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    match work {
        Work::Own => files.push((
            format!("agent-{agent}/op-{operation}.txt"),
            format!("agent {agent} operation {operation}\n").into_bytes(),
        )),
        Work::Many => {
            for number in 0..context.plan.paths_per_landing {
                files.push((
                    format!("docs/agent-{agent}/op-{operation}-{number}.txt"),
                    format!("agent {agent} operation {operation} file {number}\n").into_bytes(),
                ));
            }
        }
        Work::Shared => {
            // The draft is based on `main` as it stands at its first staging, so a marker is staged
            // first; the shared file is read after, and what it holds is at least as new as the base.
            let marker = (
                format!("agent-{agent}/marker.txt"),
                format!("operation {operation}\n").into_bytes(),
            );
            if !stage_file(session, recorder, draft, &marker).await {
                return false;
            }
            let started = Instant::now();
            let (status, bytes) = session.read("shared.txt").await;
            recorder.record(Kind::Read, started.elapsed(), status);
            if !succeeded(status) {
                return false;
            }
            let text = replace_lines(
                &String::from_utf8_lossy(&bytes),
                agent * OWNED_LINES,
                OWNED_LINES,
                &format!("agent {agent} operation {operation}"),
            );
            files.push(("shared.txt".to_owned(), text.into_bytes()));
        }
    }
    for file in &files {
        if !stage_file(session, recorder, draft, file).await {
            return false;
        }
    }
    true
}

async fn stage_file(
    session: &Session<'_>,
    recorder: &mut Recorder,
    draft: &str,
    (path, content): &(String, Vec<u8>),
) -> bool {
    let started = Instant::now();
    let status = session.stage(draft, path, content).await;
    recorder.record(Kind::Stage, started.elapsed(), status);
    succeeded(status)
}

/// `text` with the `count` lines from line `from` replaced by `count` lines that begin with
/// `label`; the other lines are as they were.
fn replace_lines(text: &str, from: usize, count: usize, label: &str) -> String {
    text.lines()
        .enumerate()
        .map(|(number, line)| {
            if (from..from + count).contains(&number) {
                format!("{label} line {number}\n")
            } else {
                format!("{line}\n")
            }
        })
        .collect()
}

/// A xorshift generator, seeded by the agent so a run's choices repeat.
struct Random(u64);

impl Random {
    fn new(agent: usize) -> Self {
        Self((agent as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % bound
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_only_the_lines_an_agent_owns() {
        let text = "a\nb\nc\nd\ne\n";
        assert_eq!(
            replace_lines(text, 1, 2, "x"),
            "a\nx line 1\nx line 2\nd\ne\n"
        );
        // The control: a range beyond the text changes nothing.
        assert_eq!(replace_lines(text, 9, 2, "x"), text);
    }

    #[test]
    fn picks_round_robin_and_never_fails_on_none() {
        let items = vec!["a".to_owned(), "b".to_owned()];
        assert_eq!(pick(&items, 0), "a");
        assert_eq!(pick(&items, 3), "b");
        assert_eq!(pick(&[], 5), "");
    }

    #[test]
    fn stays_below_its_bound_and_differs_between_agents() {
        let mut first = Random::new(0);
        let mut second = Random::new(1);
        let mut differs = false;
        for _ in 0..100 {
            let a = first.below(100);
            assert!(a < 100);
            differs |= a != second.below(100);
        }
        assert!(differs);
    }

    #[test]
    fn splits_a_url_into_origin_and_path() {
        let (origin, path) = split_origin("http://127.0.0.1:18081/realms/codehub").unwrap();
        assert_eq!(origin, "http://127.0.0.1:18081");
        assert_eq!(path, "/realms/codehub");
    }
}
