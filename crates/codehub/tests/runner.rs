//! The shipped `codehub` binary and the runner's composition site against the services the
//! checks run (`dev/check-env.sh`): a landing queues a check, a runner registered through the
//! API claims it, runs it on this machine and reports how it ended.
//!
//! The runner runs in the test process, through the function the `codehub-runner` binary calls:
//! cargo gives a package's tests the path of its own binaries only.

#[cfg(test)]
pub mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use app::domain::address::Address;
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::IntoResponse;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use clap::Parser;
use codehub_runner::Config;
use common::{Client, Server, TestDatabase, token};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

static NEXT: AtomicU64 = AtomicU64::new(1);

/// How long the runner may take to claim, run and report one job.
const RUN_BOUND: Duration = Duration::from_secs(60);

/// A server whose organisation `acme` has a repository `app` and a runner registered for it.
#[cfg(test)]
struct World {
    _database: TestDatabase,
    server: Server,
    alice: Client,
    bob_token: String,
    dir: PathBuf,
}

#[cfg(test)]
impl World {
    /// `settings` are added to the server's environment.
    async fn new(settings: &[(&str, &str)]) -> Self {
        let database = TestDatabase::new().await;
        let server = Server::start_with(&database, settings);
        let alice = Client::new(&server.base, Some(token("alice").await));
        alice.create_repository("acme", "app").await;
        let bob_token = token("bob").await;
        let (issuer, subject) = identity_of(&bob_token);
        let (status, _) = alice
            .send(
                Method::POST,
                "/v1/orgs/acme/runners",
                Some(json!({"issuer": issuer, "subject": subject, "labels": []})),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        let dir = std::env::temp_dir().join(format!(
            "codehub-runner-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("token"), &bob_token).unwrap();
        Self {
            _database: database,
            server,
            alice,
            bob_token,
            dir,
        }
    }

    /// Lands `.codehub/checks.toml` with one check `unit` that runs `command`, allowed `timeout`
    /// seconds; returns the landed changeset.
    async fn land_check(&self, command: &str, timeout: u32) -> String {
        let definitions =
            format!("[[check]]\nname = \"unit\"\ncommand = {command:?}\ntimeout = {timeout}\n");
        self.alice
            .land_files(
                "acme",
                "app",
                "work",
                &[(".codehub/checks.toml", &definitions)],
            )
            .await
    }

    /// Runs `codehub-runner --once` against `server`.
    async fn run_runner(&self, server: &str) {
        let token = self.dir.join("token");
        let work = self.dir.join("work");
        let config = Config::try_parse_from([
            "codehub-runner",
            "--server",
            server,
            "--token-file",
            token.to_str().unwrap(),
            "--work-dir",
            work.to_str().unwrap(),
            "--once",
        ])
        .unwrap();
        tokio::time::timeout(RUN_BOUND, codehub_runner::run(config))
            .await
            .expect("the runner did not finish in time")
            .unwrap();
    }

    /// The checks of `changeset`'s first check, as the API reports it.
    async fn check(&self, changeset: &str) -> Value {
        let (status, body) = self
            .alice
            .send(
                Method::GET,
                &format!("/v1/orgs/acme/repos/app/changesets/{changeset}/checks"),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        body["checks"][0].clone()
    }
}

#[cfg(test)]
impl Drop for World {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// The issuer and subject a token carries.
#[cfg(test)]
fn identity_of(token: &str) -> (String, String) {
    let payload = token.split('.').nth(1).unwrap();
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
    (
        claims["iss"].as_str().unwrap().to_owned(),
        claims["sub"].as_str().unwrap().to_owned(),
    )
}

/// Whether the process is not running: absent, or a zombie waiting to be reaped.
#[cfg(test)]
fn is_gone(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
        stat.rsplit(')')
            .next()
            .is_some_and(|rest| rest.trim_start().starts_with('Z'))
    })
}

#[cfg(test)]
async fn wait_gone(pid: u32) -> bool {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if is_gone(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn a_landing_queues_a_job_that_a_claim_returns() {
    let world = World::new(&[]).await;
    let definitions = "[[check]]\nname = \"unit\"\ncommand = \"true\"\ntimeout = 60\n";
    let changeset = world
        .alice
        .land_files(
            "acme",
            "app",
            "work",
            &[(".codehub/checks.toml", definitions)],
        )
        .await;

    let queued = world.check(&changeset).await;
    assert_eq!(queued["name"], "unit");
    assert_eq!(queued["state"], "queued");

    let bob = Client::new(&world.server.base, Some(world.bob_token.clone()));
    let (status, job) = bob.send(Method::POST, "/v1/runner/jobs/claim", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(job["organisation"], "acme");
    assert_eq!(job["changeset"], changeset);
    assert_eq!(job["check"]["name"], "unit");
    assert_eq!(job["check"]["command"], "true");
    let files = job["files"].as_array().unwrap();
    let file = files
        .iter()
        .find(|file| file["path"] == ".codehub/checks.toml")
        .unwrap();
    assert_eq!(
        file["address"],
        Address::of(definitions.as_bytes()).to_string()
    );
    assert_eq!(world.check(&changeset).await["state"], "running");
}

#[tokio::test]
async fn a_passing_check_is_reported_passed_with_its_output() {
    let world = World::new(&[]).await;
    let changeset = world
        .land_check(
            "test -f .codehub/checks.toml && echo \"ran $CODEHUB_ORGANISATION $CODEHUB_CHECK\"; echo noise >&2",
            60,
        )
        .await;

    world.run_runner(&world.server.base).await;

    let check = world.check(&changeset).await;
    assert_eq!(check["state"], "passed");
    assert_eq!(check["exit_code"], 0);
    let output = check["output_tail"].as_str().unwrap();
    assert!(output.contains("ran acme unit"), "{output}");
    assert!(output.contains("noise"), "{output}");
}

#[tokio::test]
async fn a_failing_check_is_reported_failed_with_its_exit_code() {
    let world = World::new(&[]).await;
    let changeset = world.land_check("echo broken; exit 7", 60).await;

    world.run_runner(&world.server.base).await;

    let check = world.check(&changeset).await;
    assert_eq!(check["state"], "failed");
    assert_eq!(check["exit_code"], 7);
    assert!(check["output_tail"].as_str().unwrap().contains("broken"));
}

#[tokio::test]
async fn a_check_that_outlives_its_timeout_is_stopped_with_its_process_group() {
    let world = World::new(&[
        ("CODEHUB_RUNNER_HEARTBEAT_INTERVAL", "1"),
        ("CODEHUB_RUNNER_LEASE", "5"),
    ])
    .await;
    let pidfile = world.dir.join("child.pid");
    let changeset = world
        .land_check(
            &format!("sleep 300 & echo $! > {}; wait", pidfile.display()),
            1,
        )
        .await;

    world.run_runner(&world.server.base).await;

    let check = world.check(&changeset).await;
    assert_eq!(check["state"], "timed_out");
    let pid: u32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        wait_gone(pid).await,
        "the child sleep {pid} outlived the stop"
    );
}

/// Answers every request as `upstream` does, except that a claimed job's files carry another
/// address than the one their bytes hash to. Returns the proxy's base URL.
#[cfg(test)]
async fn tampering_proxy(upstream: &str) -> String {
    async fn forward(State(upstream): State<String>, request: Request) -> impl IntoResponse {
        let (parts, body) = request.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        let path = parts.uri.path_and_query().unwrap().as_str().to_owned();
        let mut forwarded = reqwest::Client::new()
            .request(parts.method.clone(), format!("{upstream}{path}"))
            .body(bytes);
        for (name, value) in &parts.headers {
            if name != "host" {
                forwarded = forwarded.header(name, value);
            }
        }
        let response = forwarded.send().await.unwrap();
        let status = response.status();
        let mut body = response.bytes().await.unwrap().to_vec();
        if path.ends_with("/jobs/claim") && status == StatusCode::OK {
            let mut job: Value = serde_json::from_slice(&body).unwrap();
            for file in job["files"].as_array_mut().unwrap() {
                file["address"] = json!(format!("sha256:{}", "0".repeat(64)));
            }
            body = serde_json::to_vec(&job).unwrap();
        }
        (status, Body::from(body))
    }
    let app = Router::new()
        .fallback(forward)
        .with_state(upstream.to_owned());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    url
}

#[tokio::test]
async fn a_file_whose_bytes_do_not_match_its_address_fails_the_check_and_runs_no_command() {
    let world = World::new(&[]).await;
    let marker = world.dir.join("ran");
    let changeset = world
        .land_check(&format!("touch {}", marker.display()), 60)
        .await;
    let proxy = tampering_proxy(&world.server.base).await;

    world.run_runner(&proxy).await;

    let check = world.check(&changeset).await;
    assert_eq!(check["state"], "failed");
    let output = check["output_tail"].as_str().unwrap();
    assert!(output.contains(".codehub/checks.toml"), "{output}");
    assert!(output.contains("do not match its address"), "{output}");
    assert!(!marker.exists(), "the command ran");
}

#[tokio::test]
async fn refuses_to_start_where_the_lease_does_not_exceed_the_heartbeat_interval() {
    let database = TestDatabase::new().await;
    let Err(line) = Server::try_start_with(
        &database,
        &[
            ("CODEHUB_RUNNER_HEARTBEAT_INTERVAL", "5"),
            ("CODEHUB_RUNNER_LEASE", "5"),
        ],
    ) else {
        panic!("codehub started with a lease equal to the heartbeat interval");
    };
    assert!(line.contains("CODEHUB_RUNNER_LEASE"), "{line}");
}
