//! Runners, checks, `last-green` and revert through the built `codehub`, `codehub-runner` and
//! `ch`. `codehub` verifies Keycloak's tokens offline. Alice administers the organisation, and
//! the runners are Keycloak's bob and carol: each runner reads the token of its user, from the
//! password grant of the client `codehub-test`, from a file. The runner settings are short, so a
//! check's timeout is a few seconds.

#[cfg(test)]
pub mod common;

use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{Server, TestDatabase, TokenUser, binary, keycloak_token, unique};
use reqwest::{Method, StatusCode};
use serde_json::Value;
use tokio::io::AsyncReadExt;

/// How long a condition is waited for, and how long a runner may take to finish.
#[cfg(test)]
const BOUND: Duration = Duration::from_secs(60);

/// The runner's failure to claim, as `codehub-runner` prints it.
#[cfg(test)]
const CLAIM_FAILED: &str = "claiming a job failed";

/// A scratch directory: the runners' work directory, the token files and the pid file. Removed
/// when dropped, after the `sleep` a check left behind is killed.
#[cfg(test)]
struct Scratch {
    dir: PathBuf,
}

#[cfg(test)]
impl Scratch {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR"));
        let dir = parent.join(unique("runners"));
        std::fs::create_dir_all(dir.join("work")).unwrap();
        Self {
            dir: dir.canonicalize().unwrap(),
        }
    }

    fn work(&self) -> PathBuf {
        self.dir.join("work")
    }

    /// Where a check writes the pid of the `sleep` it starts.
    fn pid_file(&self) -> PathBuf {
        self.dir.join("child.pid")
    }

    /// The pid in the pid file, once the check wrote it.
    fn pid(&self) -> Option<u32> {
        std::fs::read_to_string(self.pid_file())
            .ok()?
            .trim()
            .parse()
            .ok()
    }
}

#[cfg(test)]
impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(pid) = self.pid()
            && std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .is_ok_and(|name| name.trim() == "sleep")
        {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A started `codehub-runner`, killed when dropped.
#[cfg(test)]
struct Runner {
    child: tokio::process::Child,
}

#[cfg(test)]
impl Runner {
    /// Waits for the runner to exit and returns how, with what it wrote to stderr.
    async fn finish(mut self) -> (ExitStatus, String) {
        let status = tokio::time::timeout(BOUND, self.child.wait())
            .await
            .expect("the runner did not exit within the bound")
            .unwrap();
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .await
            .unwrap();
        (status, stderr)
    }
}

/// The token file and the identity of one Keycloak user as a runner.
#[cfg(test)]
struct Identity {
    token_file: PathBuf,
    issuer: String,
    subject: String,
}

/// One server, its organisation `acme` with the repository `app`, and alice, who administers it.
#[cfg(test)]
struct World {
    _database: TestDatabase,
    server: Server,
    alice: TokenUser,
    scratch: Scratch,
}

#[cfg(test)]
impl World {
    async fn new() -> Self {
        let database = TestDatabase::new().await;
        let server = Server::start_runners(&database);
        let alice = TokenUser::new(&server, "TEST", "alice").await;
        alice.ok(&["org", "create", "acme"]).await;
        alice.ok(&["repo", "create", "acme/app"]).await;
        alice.ok(&["use", "acme/app"]).await;
        Self {
            _database: database,
            server,
            alice,
            scratch: Scratch::new(),
        }
    }

    /// Creates the repository `name` (`acme/soft`) in an organisation that exists, and returns
    /// alice with it selected.
    async fn repository(&self, name: &str) -> TokenUser {
        let alice = TokenUser::new(&self.server, "TEST", "alice").await;
        alice.ok(&["repo", "create", name]).await;
        alice.ok(&["use", name]).await;
        alice
    }

    /// The token of `user` in a file of its own, and who it names.
    async fn identity(&self, user: &str) -> Identity {
        let token = keycloak_token("TEST", user).await;
        let payload = token.split('.').nth(1).unwrap();
        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
        let token_file = self.scratch.dir.join(format!("token-{user}"));
        std::fs::write(&token_file, &token).unwrap();
        Identity {
            token_file,
            issuer: claims["iss"].as_str().unwrap().to_owned(),
            subject: claims["sub"].as_str().unwrap().to_owned(),
        }
    }

    /// Registers `runner` as a runner of the organisation `admin` has selected, with `labels`, and
    /// returns the registration's id.
    async fn register(&self, admin: &TokenUser, runner: &Identity, labels: &[&str]) -> String {
        let mut arguments = vec![
            "runner",
            "register",
            &runner.subject,
            "--issuer",
            &runner.issuer,
        ];
        for label in labels {
            arguments.extend(["--label", label]);
        }
        admin.ok(&arguments).await.trim().to_owned()
    }

    /// Starts `codehub-runner --once` as `runner`: one claim, the job it returns, then exit.
    fn run_once(&self, runner: &Identity) -> Runner {
        let child = tokio::process::Command::new(binary("codehub-runner"))
            .args(["--server", &self.server.base, "--token-file"])
            .arg(&runner.token_file)
            .arg("--work-dir")
            .arg(self.scratch.work())
            .arg("--once")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        Runner { child }
    }

    /// An authenticated request of alice's to the server: the status and the JSON body, `null`
    /// where there is none.
    async fn api(&self, method: Method, path: &str) -> (StatusCode, Value) {
        let response = reqwest::Client::new()
            .request(method, format!("{}{path}", self.server.base))
            .bearer_auth(self.alice.token())
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.json().await.unwrap_or(Value::Null);
        (status, body)
    }

    /// The changeset `main` of `acme/<repository>` names.
    async fn main_of(&self, repository: &str) -> String {
        let (status, body) = self
            .api(
                Method::GET,
                &format!("/v1/orgs/acme/repos/{repository}/refs/main"),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        body["changeset"].as_str().unwrap().to_owned()
    }
}

/// `check` as the definition in `.codehub/checks.toml` that runs `command`, with `extra` lines.
#[cfg(test)]
fn definition(name: &str, command: &str, extra: &str) -> String {
    format!("[[check]]\nname = {name:?}\ncommand = {command:?}\n{extra}\n")
}

/// Lands `files` as one changeset for `user` and returns its identifier.
#[cfg(test)]
async fn land(user: &TokenUser, files: &[(&str, &str)]) -> String {
    for (path, content) in files {
        user.stage("work", path, content).await;
    }
    user.ok(&["land", "-m", "change", "--draft", "work"])
        .await
        .trim()
        .to_owned()
}

/// Lands `definitions` as `.codehub/checks.toml` and returns the changeset.
#[cfg(test)]
async fn land_checks(user: &TokenUser, definitions: &str) -> String {
    land(user, &[(".codehub/checks.toml", definitions)]).await
}

/// `last-green` and the checks of `changeset`, as `ch checks --json` prints them.
#[cfg(test)]
async fn checks(user: &TokenUser, changeset: &str) -> Value {
    let out = user.ok(&["--json", "checks", changeset]).await;
    serde_json::from_str(&out).unwrap_or_else(|_| panic!("ch checks printed {out}"))
}

/// Polls `probe` until it returns a value, for at most [`BOUND`].
#[cfg(test)]
async fn eventually<T>(what: &str, mut probe: impl AsyncFnMut() -> Option<T>) -> T {
    let until = Instant::now() + BOUND;
    loop {
        if let Some(found) = probe().await {
            return found;
        }
        assert!(Instant::now() < until, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Waits until the first check of `changeset` is running, and returns it.
#[cfg(test)]
async fn running(user: &TokenUser, changeset: &str) -> Value {
    eventually("the check to run", async || {
        let check = checks(user, changeset).await["checks"][0].clone();
        (check["state"] == "running").then_some(check)
    })
    .await
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

/// Waits until the process `pid` is gone.
#[cfg(test)]
async fn gone(pid: u32) {
    eventually(&format!("the process {pid} to be gone"), async || {
        is_gone(pid).then_some(())
    })
    .await;
}

/// A command that starts a `sleep` in the background, writes its pid to `pid_file`, and waits on it.
#[cfg(test)]
fn sleeper(pid_file: &Path) -> String {
    format!("sleep 300 & echo $! > {}; wait", pid_file.display())
}

#[tokio::test]
async fn registers_a_runner_and_lists_it() {
    let world = World::new().await;
    let bob = world.identity("bob").await;

    let organisation = world.register(&world.alice, &bob, &["linux", "gpu"]).await;
    let repository = world
        .alice
        .ok(&[
            "runner",
            "register",
            &bob.subject,
            "--issuer",
            &bob.issuer,
            "--repo",
            "app",
        ])
        .await
        .trim()
        .to_owned();

    let listed = world.alice.ok(&["runner", "list"]).await;
    let lines: Vec<&str> = listed.lines().collect();
    assert_eq!(lines.len(), 2, "{listed}");
    let found = |id: &str| {
        lines
            .iter()
            .find(|line| line.starts_with(&format!("{id} ")))
    };
    let line = found(&organisation).unwrap();
    assert!(line.contains(" organisation "), "{line}");
    assert!(
        line.ends_with(&format!("{} {} gpu,linux", bob.issuer, bob.subject)),
        "{line}"
    );
    let line = found(&repository).unwrap();
    assert!(line.contains(" repository:"), "{line}");
    assert!(
        line.ends_with(&format!("{} {} -", bob.issuer, bob.subject)),
        "{line}"
    );

    // A registration is the administrator's to make: carol, who is not a member, may not.
    let carol = TokenUser::new(&world.server, "TEST", "carol").await;
    carol.ok(&["use", "acme/app"]).await;
    let refused = carol
        .ch(&["runner", "register", &bob.subject, "--issuer", &bob.issuer])
        .await;
    assert_eq!(refused.code, Some(5), "{}", refused.stderr);
    assert_eq!(world.alice.ok(&["runner", "list"]).await, listed);
}

#[tokio::test]
async fn a_landing_queues_its_checks() {
    let world = World::new().await;
    let alice = &world.alice;

    // A landing with no definitions queues nothing.
    let plain = land(alice, &[("a.txt", "a\n")]).await;
    assert_eq!(checks(alice, &plain).await["checks"], serde_json::json!([]));

    let definitions = [
        definition("unit", "true", "labels = [\"linux\"]"),
        definition("lint", "true", "blocking = false"),
    ]
    .concat();
    let landed = land_checks(alice, &definitions).await;

    let listed = checks(alice, &landed).await;
    let queued = listed["checks"].as_array().unwrap();
    assert_eq!(queued.len(), 2, "{listed}");
    let unit = queued.iter().find(|check| check["name"] == "unit").unwrap();
    assert_eq!(unit["state"], "queued");
    assert_eq!(unit["blocking"], true);
    assert_eq!(unit["labels"], serde_json::json!(["linux"]));
    let lint = queued.iter().find(|check| check["name"] == "lint").unwrap();
    assert_eq!(lint["state"], "queued");
    assert_eq!(lint["blocking"], false);
}

#[tokio::test]
async fn a_runner_runs_a_check_on_the_host_and_reports_its_result() {
    let world = World::new().await;
    let bob = world.identity("bob").await;
    world.register(&world.alice, &bob, &[]).await;
    let command =
        "pwd; echo \"ran $CODEHUB_ORGANISATION $CODEHUB_CHECK\"; test -f .codehub/checks.toml";
    let landed = land_checks(&world.alice, &definition("unit", command, "")).await;

    let (status, stderr) = world.run_once(&bob).finish().await;

    assert!(status.success(), "{stderr}");
    let listed = checks(&world.alice, &landed).await;
    let check = &listed["checks"][0];
    assert_eq!(check["state"], "passed", "{listed}");
    assert_eq!(check["exit_code"], 0);
    let output = check["output_tail"].as_str().unwrap();
    assert!(output.contains("ran acme unit"), "{output}");
    let directory = output.lines().next().unwrap();
    assert!(
        Path::new(directory).starts_with(world.scratch.work()),
        "the job ran in {directory}"
    );

    // The control: a command that fails is reported as failed, so passed was the command's own.
    let failing = world.repository("acme/failing").await;
    let landed = land_checks(&failing, &definition("unit", "exit 7", "blocking = false")).await;
    let (status, stderr) = world.run_once(&bob).finish().await;
    assert!(status.success(), "{stderr}");
    let check = &checks(&failing, &landed).await["checks"][0];
    assert_eq!(
        (check["state"].as_str(), check["exit_code"].as_i64()),
        (Some("failed"), Some(7))
    );
}

#[tokio::test]
async fn last_green_moves_when_every_blocking_check_passes() {
    let world = World::new().await;
    let alice = &world.alice;
    let bob = world.identity("bob").await;
    world.register(alice, &bob, &[]).await;
    let base = land(alice, &[("a.txt", "a\n")]).await;
    assert_eq!(checks(alice, &base).await["last_green"], base.as_str());
    let definitions = [
        definition("unit", "true", ""),
        definition("lint", "true", ""),
    ]
    .concat();
    let landed = land_checks(alice, &definitions).await;
    assert_eq!(checks(alice, &landed).await["last_green"], base.as_str());

    // One of the two passes: the other still blocks.
    let (status, stderr) = world.run_once(&bob).finish().await;
    assert!(status.success(), "{stderr}");
    let half = checks(alice, &landed).await;
    assert_eq!(half["last_green"], base.as_str(), "{half}");

    let (status, stderr) = world.run_once(&bob).finish().await;
    assert!(status.success(), "{stderr}");
    let done = checks(alice, &landed).await;
    assert!(
        done["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|check| check["state"] == "passed"),
        "{done}"
    );
    assert_eq!(done["last_green"], landed.as_str());
}

#[tokio::test]
async fn a_failing_blocking_check_reverts_the_landing_as_a_new_changeset() {
    let world = World::new().await;
    let alice = &world.alice;
    let bob = world.identity("bob").await;
    world.register(alice, &bob, &[]).await;
    let base = land(alice, &[("keep.txt", "keep\n")]).await;
    let landed = land(
        alice,
        &[
            ("bad.txt", "bad\n"),
            (
                ".codehub/checks.toml",
                &definition("unit", "echo broken; exit 3", ""),
            ),
        ],
    )
    .await;
    assert_eq!(world.main_of("app").await, landed);

    let (status, stderr) = world.run_once(&bob).finish().await;

    assert!(status.success(), "{stderr}");
    let listed = checks(alice, &landed).await;
    let check = &listed["checks"][0];
    assert_eq!(check["state"], "failed", "{listed}");
    assert_eq!(check["exit_code"], 3);
    assert_eq!(check["revert"]["outcome"], "landed", "{listed}");
    let revert = check["revert"]["changeset"].as_str().unwrap();
    assert_ne!(revert, landed);
    assert_ne!(revert, base);
    // The revert is the head of main, a new changeset on top of the landing.
    assert_eq!(world.main_of("app").await, revert);
    assert_eq!(alice.ch(&["read", "bad.txt"]).await.code, Some(5));
    assert_eq!(alice.ok(&["read", "keep.txt"]).await, "keep\n");
    // `last-green` skipped the landing that failed: the revert carries no blocking check, so it
    // is the revert that is green.
    assert_ne!(listed["last_green"], landed.as_str());
    assert_eq!(listed["last_green"], revert);
}

#[tokio::test]
async fn a_failing_non_blocking_check_reverts_nothing() {
    let world = World::new().await;
    let bob = world.identity("bob").await;
    world.register(&world.alice, &bob, &[]).await;
    let soft = world.repository("acme/soft").await;
    let soft_landed = land(
        &soft,
        &[
            ("soft.txt", "soft\n"),
            (
                ".codehub/checks.toml",
                &definition("unit", "exit 1", "blocking = false"),
            ),
        ],
    )
    .await;

    let (status, stderr) = world.run_once(&bob).finish().await;

    assert!(status.success(), "{stderr}");
    let listed = checks(&soft, &soft_landed).await;
    assert_eq!(listed["checks"][0]["state"], "failed", "{listed}");
    assert_eq!(listed["checks"][0]["revert"], Value::Null);
    assert_eq!(world.main_of("soft").await, soft_landed);
    assert_eq!(soft.ok(&["read", "soft.txt"]).await, "soft\n");

    // The control: the same command in a blocking check reverts its landing.
    let hard = world.repository("acme/hard").await;
    let hard_landed = land(
        &hard,
        &[
            ("hard.txt", "hard\n"),
            (".codehub/checks.toml", &definition("unit", "exit 1", "")),
        ],
    )
    .await;
    let (status, stderr) = world.run_once(&bob).finish().await;
    assert!(status.success(), "{stderr}");
    let listed = checks(&hard, &hard_landed).await;
    assert_eq!(
        listed["checks"][0]["revert"]["outcome"], "landed",
        "{listed}"
    );
    assert_ne!(world.main_of("hard").await, hard_landed);
}

#[tokio::test]
async fn a_job_goes_only_to_a_runner_with_matching_labels() {
    let world = World::new().await;
    let alice = &world.alice;
    let (bob, carol) = (world.identity("bob").await, world.identity("carol").await);
    world.register(alice, &bob, &["cpu"]).await;
    world.register(alice, &carol, &["cpu", "gpu"]).await;
    let landed = land_checks(alice, &definition("unit", "true", "labels = [\"gpu\"]")).await;

    // bob offers cpu only: his claim is answered with no job.
    let (status, stderr) = world.run_once(&bob).finish().await;
    assert!(status.success(), "{stderr}");
    assert!(!stderr.contains("started"), "{stderr}");
    assert_eq!(checks(alice, &landed).await["checks"][0]["state"], "queued");

    // The control: carol offers gpu as well, and the same job goes to her.
    let (status, stderr) = world.run_once(&carol).finish().await;
    assert!(status.success(), "{stderr}");
    assert_eq!(checks(alice, &landed).await["checks"][0]["state"], "passed");
}

#[tokio::test]
async fn a_job_past_its_timeout_is_stopped_and_reported_timed_out() {
    let world = World::new().await;
    let alice = &world.alice;
    let bob = world.identity("bob").await;
    world.register(alice, &bob, &[]).await;
    let command = sleeper(&world.scratch.pid_file());
    let landed = land_checks(alice, &definition("unit", &command, "timeout = 2")).await;
    let runner = world.run_once(&bob);

    // The check is running and its `sleep` is alive before the timeout stops them.
    running(alice, &landed).await;
    let pid = eventually("the check to write its pid", async || world.scratch.pid()).await;
    assert!(!is_gone(pid), "the sleep {pid} is not running");
    let waited = alice
        .ch(&[
            "--json",
            "checks",
            &landed,
            "--wait",
            "--timeout",
            "60",
            "--poll-interval-ms",
            "200",
        ])
        .await;
    assert_eq!(waited.code, Some(4), "{}", waited.stderr);
    let listed: Value = serde_json::from_str(&waited.out()).unwrap();
    assert_eq!(listed["checks"][0]["state"], "timed_out", "{listed}");

    let (status, stderr) = runner.finish().await;
    assert!(status.success(), "{stderr}");
    gone(pid).await;
}

#[tokio::test]
async fn a_cancelled_job_is_stopped_and_reverts_nothing() {
    let world = World::new().await;
    let alice = &world.alice;
    let bob = world.identity("bob").await;
    world.register(alice, &bob, &[]).await;
    let base = land(alice, &[("keep.txt", "keep\n")]).await;
    let command = sleeper(&world.scratch.pid_file());
    let landed = land(
        alice,
        &[
            ("work.txt", "work\n"),
            (
                ".codehub/checks.toml",
                &definition("unit", &command, "timeout = 120"),
            ),
        ],
    )
    .await;
    let runner = world.run_once(&bob);
    let id = running(alice, &landed).await["id"].as_u64().unwrap();
    let pid = eventually("the check to write its pid", async || world.scratch.pid()).await;
    assert!(!is_gone(pid), "the sleep {pid} is not running");

    let (status, _) = world
        .api(
            Method::POST,
            &format!("/v1/orgs/acme/repos/app/jobs/{id}/cancel"),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The next heartbeat answers stop: the runner stops the job, reports and exits.
    let (exit, stderr) = runner.finish().await;
    assert!(exit.success(), "{stderr}");
    gone(pid).await;
    let listed = checks(alice, &landed).await;
    assert_eq!(listed["checks"][0]["state"], "cancelled", "{listed}");
    assert_eq!(listed["checks"][0]["revert"], Value::Null);
    assert_eq!(world.main_of("app").await, landed);
    assert_eq!(alice.ok(&["read", "work.txt"]).await, "work\n");
    assert_eq!(listed["last_green"], base.as_str());

    // The control: a blocking check that fails does revert its landing, so the absence of a
    // revert above was the cancellation's.
    let failing = world.repository("acme/failing").await;
    let broken = land_checks(&failing, &definition("unit", "exit 1", "")).await;
    let (exit, stderr) = world.run_once(&bob).finish().await;
    assert!(exit.success(), "{stderr}");
    let listed = checks(&failing, &broken).await;
    assert_eq!(
        listed["checks"][0]["revert"]["outcome"], "landed",
        "{listed}"
    );
}

#[tokio::test]
async fn a_runner_of_another_organisation_is_refused() {
    let world = World::new().await;
    let alice = &world.alice;
    alice.ok(&["org", "create", "other"]).await;
    let other = world.repository("other/app").await;
    let (bob, carol) = (world.identity("bob").await, world.identity("carol").await);
    // bob is a runner of `other`; carol is a runner of nothing.
    world.register(&other, &bob, &[]).await;
    let landed = land_checks(alice, &definition("unit", "true", "")).await;

    let (status, stderr) = world.run_once(&carol).finish().await;
    assert!(!status.success(), "{stderr}");
    assert!(stderr.contains(CLAIM_FAILED), "{stderr}");

    let (status, stderr) = world.run_once(&bob).finish().await;
    assert!(status.success(), "{stderr}");
    assert!(!stderr.contains("started"), "{stderr}");
    assert_eq!(checks(alice, &landed).await["checks"][0]["state"], "queued");

    // The control: registered in `acme` as well, bob takes the job.
    world.register(alice, &bob, &[]).await;
    let (status, stderr) = world.run_once(&bob).finish().await;
    assert!(status.success(), "{stderr}");
    assert_eq!(checks(alice, &landed).await["checks"][0]["state"], "passed");
}
