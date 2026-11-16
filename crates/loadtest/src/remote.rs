//! The load of a fault run against a server that is already running, such as the chart in a
//! cluster. Agents land small files and survive every failure; each acknowledged landing is
//! recorded, and once the load stops it is read back from the server: an acknowledged landing that
//! is missing, or differs, is the loss a fault run exists to find.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Args;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::task::JoinSet;

use crate::Failure;
use crate::api::{Api, Session, Version, address};
use crate::services;
use crate::stats::succeeded;

/// Concurrent read-backs while verifying.
const VERIFY_WIDTH: usize = 16;

/// How many times a read-back that could not reach the server is tried, a second apart.
const VERIFY_ATTEMPTS: u32 = 120;

/// The options of a run against a running server.
#[derive(Args)]
pub struct Options {
    /// The base URL of the server; the run is against it instead of local replicas.
    #[arg(long, requires = "token_url")]
    pub server: Option<String>,
    /// The token endpoint of the identity provider; the password grant is used.
    #[arg(long)]
    token_url: Option<String>,
    /// The client id for the token; its secret is in `LOADTEST_CLIENT_SECRET`.
    #[arg(long, default_value = "codehub-test")]
    client_id: String,
    /// The scope the token asks for.
    #[arg(long, default_value = "openid")]
    scope: String,
    /// The user to land as; the password is in `LOADTEST_PASSWORD`.
    #[arg(long, default_value = "alice@example.test")]
    user: String,
    /// The organisation and repository to land into; created where they do not exist.
    #[arg(long, default_value = "acme")]
    organisation: String,
    /// The repository to land into.
    #[arg(long, default_value = "app")]
    repository: String,
    /// A name for this run; every path and draft it makes starts with it, so runs do not collide.
    #[arg(long, default_value = "run")]
    label: String,
    /// Appends each acknowledged landing to this file, one JSON object a line.
    #[arg(long)]
    record: Option<PathBuf>,
    /// A shell command run once, `--fault-after` seconds into the load.
    #[arg(long, requires = "fault_after")]
    fault: Option<String>,
    /// Seconds of load before the fault.
    #[arg(long)]
    fault_after: Option<u64>,
    /// The longest, in seconds after the fault, until a landing succeeds again.
    #[arg(long, default_value_t = 60)]
    recover_within: u64,
    /// Prints an access token, for a client that is not this one, and does nothing else.
    #[arg(long, requires = "server")]
    pub print_token: bool,
    /// Does not load: reads back every landing recorded in this file.
    #[arg(long, requires = "server", conflicts_with_all = ["fault", "print_token"])]
    pub verify: Option<PathBuf>,
    /// With `--verify`, a recorded landing that is absent from `main` is accepted, as one landed
    /// after a backup is; one that is present must still be intact.
    #[arg(long, requires = "verify")]
    tolerate_missing: bool,
}

/// One acknowledged landing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ack {
    /// The file the landing wrote.
    pub path: String,
    /// What it wrote.
    pub content: String,
    /// The changeset the server acknowledged it with.
    pub changeset: String,
}

/// One request of the load and when it ended, in seconds from the start.
#[derive(Clone, Copy, Debug)]
pub struct Event {
    at: f64,
    ok: bool,
    /// The first status that was not a success; 0 where the request got no answer.
    status: u16,
}

/// The failed attempts counted by status, in the order of the status, as `503 x 12, 500 x 3`.
pub fn failures_by_status(events: &[Event]) -> String {
    let mut counts = std::collections::BTreeMap::new();
    for event in events.iter().filter(|event| !event.ok) {
        *counts.entry(event.status).or_insert(0_usize) += 1;
    }
    if counts.is_empty() {
        return "none".to_owned();
    }
    counts
        .iter()
        .map(|(status, count)| format!("{status} x {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The failed attempts counted per window of 10 seconds from the start, as `20s 31, 30s 112`;
/// windows without a failure are left out.
pub fn failures_by_window(events: &[Event]) -> String {
    let mut counts = std::collections::BTreeMap::new();
    for event in events.iter().filter(|event| !event.ok) {
        *counts.entry((event.at / 10.0) as u64 * 10).or_insert(0_usize) += 1;
    }
    if counts.is_empty() {
        return "none".to_owned();
    }
    counts
        .iter()
        .map(|(start, count)| format!("{start}s {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether a failed attempt says the system was unavailable: no answer, or a 5xx. A 409 is the
/// answer of a landing that lost the race for `main`, which the load causes with no fault and
/// which the client does not retry; a 429 is the rate limit.
fn unavailable(event: &Event) -> bool {
    !event.ok && (event.status == 0 || event.status >= 500)
}

/// Seconds from the fault until landings succeed for good: from `fault_at` to the first success
/// after the last unavailable answer (`unavailable`). Zero where none came after the fault; none
/// where the last request was unavailable, so the system did not recover.
pub fn recovery(events: &[Event], fault_at: f64) -> Option<f64> {
    let mut after: Vec<&Event> = events.iter().filter(|event| event.at >= fault_at).collect();
    after.sort_by(|left, right| left.at.total_cmp(&right.at));
    let Some(last_failure) = after.iter().rposition(|event| unavailable(event)) else {
        return Some(0.0);
    };
    after
        .iter()
        .skip(last_failure)
        .find(|event| event.ok)
        .map(|event| event.at - fault_at)
}

/// What reading one acknowledged landing back found.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The changeset is in the file's history with the content that was landed.
    Intact,
    /// The changeset is not in the file's history: the landing was lost.
    Missing,
    /// The changeset is there with other content.
    Changed,
}

/// Whether `versions`, the history of `ack.path`, hold the landing `ack`.
pub fn judge(ack: &Ack, versions: &[Version]) -> Verdict {
    match versions
        .iter()
        .find(|version| version.changeset == ack.changeset)
    {
        None => Verdict::Missing,
        Some(version)
            if version.content.as_deref() == Some(address(ack.content.as_bytes()).as_str()) =>
        {
            Verdict::Intact
        }
        Some(_) => Verdict::Changed,
    }
}

struct Context {
    api: Api,
    base: String,
    token: String,
    organisation: String,
    repository: String,
}

impl Context {
    fn session(&self) -> Session<'_> {
        self.api.session(
            &self.base,
            &self.token,
            &self.organisation,
            &self.repository,
        )
    }
}

async fn context(options: &Options) -> Result<Context, Failure> {
    let (Some(base), Some(url)) = (&options.server, &options.token_url) else {
        return Err(Failure::new("--server and --token-url go together"));
    };
    let token = services::password_token(
        url,
        &options.client_id,
        &services::env("LOADTEST_CLIENT_SECRET")?,
        &options.user,
        &services::env("LOADTEST_PASSWORD")?,
        &options.scope,
    )
    .await?;
    Ok(Context {
        api: Api::new(),
        base: base.trim_end_matches('/').to_owned(),
        token,
        organisation: options.organisation.clone(),
        repository: options.repository.clone(),
    })
}

/// Creates the organisation and the repository where they are not there yet: a 409 says they are.
async fn prepare(context: &Context) -> Result<(), Failure> {
    let session = context.session();
    let steps = [
        ("orgs".to_owned(), json!({ "name": context.organisation })),
        (
            format!("orgs/{}/repos", context.organisation),
            json!({ "name": context.repository }),
        ),
    ];
    for (url, body) in steps {
        let status = session.administer(Method::POST, &url, &body).await;
        if !succeeded(status) && status != 409 {
            return Err(Failure::new(format!("creating {url} answered {status}")));
        }
    }
    Ok(())
}

/// One agent: lands a new file after another until `deadline`, and records every outcome.
async fn agent(
    context: Arc<Context>,
    label: String,
    index: usize,
    started: Instant,
    deadline: Instant,
) -> (Vec<Ack>, Vec<Event>) {
    let mut acks = Vec::new();
    let mut events = Vec::new();
    let mut number = 0_u64;
    while Instant::now() < deadline {
        number += 1;
        let draft = format!("{label}-{index}-{number}");
        let path = format!("{label}/agent-{index}/file-{number}.txt");
        let content = format!("{label} agent {index} landing {number}\n");
        let session = context.session();
        let mut changeset = None;
        let staged = session.stage(&draft, &path, content.as_bytes()).await;
        let mut status = staged;
        if succeeded(staged) {
            let landing = session
                .land(&draft, &format!("{label} {index} {number}"))
                .await;
            status = landing.status;
            changeset = landing.changeset;
        }
        let ok = changeset.is_some();
        events.push(Event {
            at: started.elapsed().as_secs_f64(),
            ok,
            status,
        });
        if let Some(changeset) = changeset {
            acks.push(Ack {
                path,
                content,
                changeset,
            });
        } else {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    (acks, events)
}

/// Runs `command` in a shell; whether it succeeded.
async fn shell(command: &str) -> Result<(), Failure> {
    let status = tokio::process::Command::new("sh")
        .args(["-c", command])
        .status()
        .await?;
    if status.success() {
        Ok(())
    } else {
        Err(Failure::new(format!("the fault command failed: {status}")))
    }
}

/// Reads `acks` back from the server, the history and the file's bytes, and counts the verdicts:
/// intact, missing and changed.
async fn read_back(context: &Arc<Context>, acks: Vec<Ack>) -> Result<[u64; 3], Failure> {
    let mut counts = [0_u64; 3];
    let mut pending = acks.into_iter();
    let mut running = JoinSet::new();
    loop {
        while running.len() < VERIFY_WIDTH {
            let Some(ack) = pending.next() else { break };
            let context = context.clone();
            running.spawn(async move {
                for _ in 0..VERIFY_ATTEMPTS {
                    let (status, versions) = context.session().versions(&ack.path).await;
                    if succeeded(status) {
                        let verdict = judge(&ack, &versions);
                        if verdict != Verdict::Intact {
                            return Some(verdict);
                        }
                        // The history names the object; reading the file proves it is still there.
                        // A read the server did not answer is tried again, not judged.
                        let (status, bytes) = context.session().read(&ack.path).await;
                        if succeeded(status) {
                            return Some(if bytes == ack.content.as_bytes() {
                                Verdict::Intact
                            } else {
                                Verdict::Changed
                            });
                        }
                        if status == 404 {
                            return Some(Verdict::Missing);
                        }
                    } else if status == 404 {
                        return Some(Verdict::Missing);
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                None
            });
        }
        let Some(joined) = running.join_next().await else {
            return Ok(counts);
        };
        let verdict = joined
            .map_err(|error| Failure::new(format!("a read-back failed: {error}")))?
            .ok_or_else(|| Failure::new("the server did not answer a read-back in time"))?;
        let slot = match verdict {
            Verdict::Intact => 0,
            Verdict::Missing => 1,
            Verdict::Changed => 2,
        };
        if let Some(count) = counts.get_mut(slot) {
            *count += 1;
        }
    }
}

fn read_record(path: &Path) -> Result<Vec<Ack>, Failure> {
    std::fs::read_to_string(path)?
        .lines()
        .map(|line| serde_json::from_str(line).map_err(Failure::from))
        .collect()
}

fn append_record(path: &Path, acks: &[Ack]) -> Result<(), Failure> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    for ack in acks {
        writeln!(file, "{}", serde_json::to_string(ack)?)?;
    }
    Ok(())
}

/// Whether a read-back that counted `counts` (intact, missing, changed) passes.
pub const fn passes(counts: [u64; 3], tolerate_missing: bool) -> bool {
    let [intact, missing, changed] = counts;
    changed == 0 && (tolerate_missing || missing == 0) && intact + missing + changed > 0
}

/// Prints an access token for the user the options name.
pub async fn print_token(options: &Options) -> Result<bool, Failure> {
    println!("{}", context(options).await?.token);
    Ok(true)
}

/// Reads the landings in `options.verify` back; whether they pass.
pub async fn verify(options: &Options, path: &Path) -> Result<bool, Failure> {
    let acks = read_record(path)?;
    let context = Arc::new(context(options).await?);
    let total = acks.len();
    let counts = read_back(&context, acks).await?;
    let [intact, missing, changed] = counts;
    println!(
        "verify {}: {total} recorded, {intact} intact, {missing} missing, {changed} changed (missing {})",
        path.display(),
        if options.tolerate_missing {
            "tolerated"
        } else {
            "not tolerated"
        },
    );
    Ok(passes(counts, options.tolerate_missing))
}

/// Loads the server for `seconds` with `agents`, runs the fault, and reads every acknowledged
/// landing back; whether nothing was lost and the server recovered in time.
pub async fn run(options: &Options, agents: u32, seconds: u64) -> Result<bool, Failure> {
    let context = Arc::new(context(options).await?);
    prepare(&context).await?;
    let started = Instant::now();
    let deadline = started + Duration::from_secs(seconds);
    let mut running = JoinSet::new();
    for index in 0..agents as usize {
        running.spawn(agent(
            context.clone(),
            options.label.clone(),
            index,
            started,
            deadline,
        ));
    }
    let fault = async {
        let (Some(command), Some(after)) = (&options.fault, options.fault_after) else {
            return Ok(None);
        };
        tokio::time::sleep(Duration::from_secs(after)).await;
        let at = started.elapsed().as_secs_f64();
        shell(command).await?;
        Ok::<_, Failure>(Some(at))
    };
    let (fault_at, joined) = tokio::join!(fault, async {
        let mut acks = Vec::new();
        let mut events = Vec::new();
        while let Some(done) = running.join_next().await {
            let (more_acks, more_events) =
                done.map_err(|error| Failure::new(format!("an agent failed: {error}")))?;
            acks.extend(more_acks);
            events.extend(more_events);
        }
        Ok::<_, Failure>((acks, events))
    });
    let fault_at = fault_at?;
    let (acks, events) = joined?;
    let failed = events.iter().filter(|event| !event.ok).count();
    println!(
        "load {}: {} acknowledged landings, {failed} failed attempts, {agents} agents, {seconds}s",
        options.label,
        acks.len(),
    );
    println!("failed attempts by status: {}", failures_by_status(&events));
    println!("failed attempts per 10 s: {}", failures_by_window(&events));
    if let Some(path) = &options.record {
        append_record(path, &acks)?;
    }
    let mut passed = true;
    if let Some(at) = fault_at {
        let limit = options.recover_within as f64;
        match recovery(&events, at) {
            Some(took) if took <= limit => {
                println!(
                    "recovery: landings succeeded again {took:.1}s after the fault at {at:.1}s (limit {limit:.0}s)"
                );
            }
            Some(took) => {
                println!(
                    "recovery: too slow, {took:.1}s after the fault at {at:.1}s (limit {limit:.0}s)"
                );
                passed = false;
            }
            None => {
                println!("recovery: landings had not succeeded again when the load stopped");
                passed = false;
            }
        }
    }
    let total = acks.len();
    let counts = read_back(&context, acks).await?;
    let [intact, missing, changed] = counts;
    println!(
        "read-back: {total} acknowledged, {intact} intact, {missing} missing, {changed} changed"
    );
    Ok(passed && passes(counts, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(at: f64, ok: bool) -> Event {
        Event { at, ok, status: 0 }
    }

    fn ack() -> Ack {
        Ack {
            path: "run/file.txt".to_owned(),
            content: "hello\n".to_owned(),
            changeset: "sha256:aa".to_owned(),
        }
    }

    fn version(changeset: &str, content: &[u8]) -> Version {
        Version {
            changeset: changeset.to_owned(),
            content: Some(address(content)),
        }
    }

    #[test]
    fn recovery_runs_from_the_fault_to_the_first_success_after_the_last_failure() {
        let events = [
            event(1.0, true),
            event(10.0, false),
            event(11.0, true),
            event(12.0, false),
            event(15.0, true),
            event(16.0, true),
        ];
        assert_eq!(recovery(&events, 10.0), Some(5.0));
    }

    #[test]
    fn failures_are_counted_by_status() {
        let failed = |status| Event {
            at: 1.0,
            ok: false,
            status,
        };
        let events = [failed(503), failed(500), failed(503), event(2.0, true)];
        assert_eq!(failures_by_status(&events), "500 x 1, 503 x 2");
        assert_eq!(failures_by_status(&[event(2.0, true)]), "none");
    }

    #[test]
    fn failures_are_counted_per_window_of_ten_seconds() {
        let events = [
            event(1.0, true),
            event(21.0, false),
            event(29.0, false),
            event(31.0, false),
            event(32.0, true),
        ];
        assert_eq!(failures_by_window(&events), "20s 2, 30s 1");
        assert_eq!(failures_by_window(&[event(2.0, true)]), "none");
    }

    #[test]
    fn a_conflict_after_the_fault_is_not_an_outage() {
        let conflict = Event {
            at: 20.0,
            ok: false,
            status: 409,
        };
        let outage = Event {
            at: 12.0,
            ok: false,
            status: 500,
        };
        let events = [outage, event(14.0, true), conflict, event(21.0, true)];
        assert_eq!(recovery(&events, 10.0), Some(4.0));
    }

    #[test]
    fn recovery_is_zero_where_nothing_failed_after_the_fault() {
        let events = [event(1.0, false), event(10.0, true), event(11.0, true)];
        assert_eq!(recovery(&events, 5.0), Some(0.0));
    }

    #[test]
    fn recovery_is_none_where_the_last_request_failed() {
        let events = [event(10.0, true), event(11.0, false)];
        assert_eq!(recovery(&events, 5.0), None);
    }

    #[test]
    fn a_landing_in_the_history_with_its_content_is_intact() {
        let versions = [
            version("sha256:bb", b"other"),
            version("sha256:aa", b"hello\n"),
        ];
        assert_eq!(judge(&ack(), &versions), Verdict::Intact);
    }

    #[test]
    fn a_landing_absent_from_the_history_is_missing() {
        assert_eq!(
            judge(&ack(), &[version("sha256:bb", b"hello\n")]),
            Verdict::Missing
        );
        assert_eq!(judge(&ack(), &[]), Verdict::Missing);
    }

    #[test]
    fn a_landing_with_other_content_is_changed() {
        assert_eq!(
            judge(&ack(), &[version("sha256:aa", b"other")]),
            Verdict::Changed
        );
    }

    #[test]
    fn a_missing_landing_fails_unless_missing_is_tolerated() {
        assert!(!passes([9, 1, 0], false));
        assert!(passes([9, 1, 0], true));
        assert!(passes([10, 0, 0], false));
    }

    #[test]
    fn a_changed_landing_and_an_empty_read_back_fail_even_where_missing_is_tolerated() {
        assert!(!passes([9, 0, 1], true));
        assert!(!passes([0, 0, 0], true));
    }

    #[test]
    fn a_record_reads_back_what_was_appended() {
        let dir = std::env::temp_dir().join(format!("loadtest-record-{}", std::process::id()));
        let path = dir.with_extension("jsonl");
        let _ = std::fs::remove_file(&path);
        append_record(&path, &[ack()]).unwrap_or_else(|error| panic!("{error}"));
        append_record(&path, &[ack()]).unwrap_or_else(|error| panic!("{error}"));
        let read = read_record(&path).unwrap_or_else(|error| panic!("{error}"));
        let _ = std::fs::remove_file(&path);
        assert_eq!(read, vec![ack(), ack()]);
    }
}
