//! `ch run` and `ch fetch` (ADR 0051) through the built `codehub`, `codehub-runner` and `ch`. The
//! runner is Keycloak's bob, on the host executor, and runs `cargo` on a small Rust repository
//! the test lands. Alice administers the organisation and holds `run`; carol is a member without
//! it. Each test prints the evidence of its step: exit codes, object counts and what ran.

#[cfg(test)]
pub mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{Server, TestDatabase, TokenUser, binary, keycloak_token, unique};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// How long a condition is waited for.
#[cfg(test)]
const BOUND: Duration = Duration::from_secs(60);

/// The crate every test runs `cargo` on: a library with one passing test and a binary, with no
/// dependencies. `[workspace]` keeps cargo from finding the manifest of the repository this test
/// is built in.
#[cfg(test)]
const FILES: [(&str, &str); 5] = [
    (
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    ),
    (
        "src/lib.rs",
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn adds() {\n        assert_eq!(add(1, 2), 3);\n    }\n}\n",
    ),
    (
        "src/main.rs",
        "fn main() {\n    println!(\"sum {}\", demo::add(2, 3));\n}\n",
    ),
    (".codehub/run.toml", "caches = [\"target\"]\n"),
    ("note.txt", "base\n"),
];

/// A scratch directory: the runner's work directory, the token files and the views. Removed when
/// dropped.
#[cfg(test)]
struct Scratch(PathBuf);

#[cfg(test)]
impl Scratch {
    fn new() -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(unique("runs"));
        std::fs::create_dir_all(dir.join("work")).unwrap();
        Self(dir.canonicalize().unwrap())
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

#[cfg(test)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A started `codehub-runner`, killed when dropped.
#[cfg(test)]
struct Runner(#[expect(dead_code, reason = "held for kill_on_drop")] tokio::process::Child);

/// One server with the organisation `acme` and its repository `app`, holding [`FILES`] on `main`;
/// alice, who administers it, with a view of her draft `default`; and a host runner.
#[cfg(test)]
struct World {
    _database: TestDatabase,
    server: Server,
    alice: TokenUser,
    scratch: Scratch,
    view: PathBuf,
    _runner: Runner,
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
        for (path, content) in FILES {
            alice.stage("default", path, content).await;
        }
        alice.ok(&["land", "-m", "base"]).await;

        let scratch = Scratch::new();
        let (subject, issuer) = identity(&scratch, "bob").await;
        alice
            .ok(&["runner", "register", &subject, "--issuer", &issuer])
            .await;
        // `cargo` writes where it is told to: the target directory of this test's build would
        // take the job's `target` away from the cache paths.
        let runner = tokio::process::Command::new(binary("codehub-runner"))
            .args(["--server", &server.base, "--token-file"])
            .arg(scratch.join("token-bob"))
            .arg("--work-dir")
            .arg(scratch.join("work"))
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET_DIR")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();

        let view = scratch.join("view");
        alice.ok(&["open", view.to_str().unwrap()]).await;
        for (path, content) in FILES {
            assert_eq!(
                std::fs::read_to_string(view.join(path)).unwrap(),
                content,
                "the view holds {path} as landed"
            );
        }
        Self {
            _database: database,
            server,
            alice,
            scratch,
            view,
            _runner: Runner(runner),
        }
    }

    /// `ch run -- <command>` as alice in her view.
    async fn run(&self, command: &str) -> common::Run {
        self.alice.ch_in(&self.view, &["run", "--", command]).await
    }

    /// `ch run --json -- <command>` as alice in her view: the exit code and the run as printed.
    async fn run_json(&self, command: &str) -> (Option<i32>, Value) {
        let run = self
            .alice
            .ch_in(&self.view, &["run", "--json", "--", command])
            .await;
        let printed = run.out();
        let body = serde_json::from_str(&printed)
            .unwrap_or_else(|_| panic!("ch run --json printed {printed}: {}", run.stderr));
        (run.code, body)
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.view.join(path)).unwrap()
    }

    fn write(&self, path: &str, content: &str) {
        std::fs::write(self.view.join(path), content).unwrap();
    }
}

/// The subject and issuer of Keycloak's `user`, whose token is written to `token-<user>` in
/// `scratch`.
#[cfg(test)]
async fn identity(scratch: &Scratch, user: &str) -> (String, String) {
    let token = keycloak_token("TEST", user).await;
    let payload = token.split('.').nth(1).unwrap();
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
    std::fs::write(scratch.join(&format!("token-{user}")), &token).unwrap();
    (
        claims["sub"].as_str().unwrap().to_owned(),
        claims["iss"].as_str().unwrap().to_owned(),
    )
}

/// The object counts of a run as `ch run --json` prints it: downloaded and from the cache.
#[cfg(test)]
fn objects(run: &Value) -> (u64, u64) {
    let counts = &run["result"]["objects"];
    (
        counts["downloaded"].as_u64().unwrap(),
        counts["from_cache"].as_u64().unwrap(),
    )
}

#[tokio::test]
async fn runs_the_tests_on_the_runner_and_reports_a_failing_one() {
    let world = World::new().await;

    let passed = world.run("cargo test").await;
    eprintln!(
        "step 1: `ch run -- cargo test` exit {:?}; output:\n{}",
        passed.code,
        passed.out()
    );
    assert_eq!(passed.code, Some(0), "{}", passed.stderr);
    assert!(
        passed.out().contains("test tests::adds ... ok"),
        "{}",
        passed.out()
    );
    assert!(passed.stderr.ends_with("exit 0\n"), "{}", passed.stderr);

    // The failing test is an edit of the view, which `ch run` sends before it runs.
    let failing = world.read("src/lib.rs").replace("(1, 2), 3", "(1, 2), 4");
    assert_ne!(failing, world.read("src/lib.rs"), "the test was changed");
    world.write("src/lib.rs", &failing);
    // The control: the edit reached the runner, so a pass below would be cargo's.
    let sent = world.run("grep -c '(1, 2), 4' src/lib.rs").await;
    eprintln!(
        "step 1: the runner's copy holds the edit: exit {:?}",
        sent.code
    );
    assert_eq!(sent.code, Some(0), "{}", sent.stderr);
    let failed = world.run("cargo test").await;
    eprintln!(
        "step 1: after a failing test, `ch run -- cargo test` exit {:?}; output:\n{}",
        failed.code,
        failed.out()
    );
    assert_eq!(failed.code, Some(101), "{}", failed.stderr);
    assert!(
        failed.out().contains("test tests::adds ... FAILED"),
        "{}",
        failed.out()
    );
    assert!(failed.stderr.ends_with("exit 101\n"), "{}", failed.stderr);
}

#[tokio::test]
async fn writes_the_formatted_file_into_the_draft_and_the_view() {
    let world = World::new().await;
    let landed = world.read("src/lib.rs");
    world.write(
        "src/lib.rs",
        &format!("{landed}\npub fn double(x:i32)->i32{{x*2}}\n"),
    );
    let formatted = format!("{landed}\npub fn double(x: i32) -> i32 {{\n    x * 2\n}}\n");

    let run = world.run("cargo fmt").await;
    eprintln!(
        "step 2: `ch run -- cargo fmt` exit {:?}; stderr:\n{}",
        run.code, run.stderr
    );
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(world.read("src/lib.rs"), formatted);
    // The write-back reached the draft, not only the view: `ch status` compares the view with
    // the last snapshot, and `ch diff` the draft with `main`.
    let status = world.alice.ch_in(&world.view, &["status"]).await.out();
    let diff = world.alice.ch_in(&world.view, &["diff"]).await.out();
    eprintln!("step 2: view status:\n{status}draft diff:\n{diff}");
    assert!(diff.contains("src/lib.rs"), "{diff}");
    assert!(diff.contains("+pub fn double(x: i32) -> i32 {"), "{diff}");

    // The control: a run that changes nothing writes nothing back, so the file above came from
    // `cargo fmt`.
    let before = world.read("src/lib.rs");
    let unchanged = world.run("cargo fmt").await;
    assert_eq!(unchanged.code, Some(0), "{}", unchanged.stderr);
    assert_eq!(world.read("src/lib.rs"), before);
}

#[tokio::test]
async fn refuses_to_write_back_into_a_draft_that_changed_during_the_run() {
    let world = World::new().await;
    let started = world.scratch.join("started");
    let command = format!("echo ran > note.txt; touch {}; sleep 4", started.display());
    // The control: with nothing changing the draft meanwhile, the same command is written back.
    let (code, run) = world.run_json(&command).await;
    eprintln!(
        "step 3 control: exit {code:?}, write_back {}",
        run["result"]["write_back"]
    );
    assert_eq!(code, Some(0));
    assert_eq!(world.read("note.txt"), "ran\n");
    std::fs::remove_file(&started).unwrap();

    world.write("note.txt", "base\n");
    let arguments = ["run", "--json", "--", &command];
    let running = world.alice.ch_in(&world.view, &arguments);
    let change = async {
        let until = Instant::now() + BOUND;
        while !started.exists() {
            assert!(Instant::now() < until, "the run did not start");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // Another change to the same draft, made while the command sleeps.
        world
            .alice
            .stage("default", "other.txt", "changed meanwhile\n")
            .await;
    };
    let (run, ()) = tokio::join!(running, change);
    let printed = run.out();
    let body: Value = serde_json::from_str(&printed).unwrap();
    eprintln!(
        "step 3: exit {:?}, write_back {}, stderr:\n{}",
        run.code, body["result"]["write_back"], run.stderr
    );
    assert_eq!(run.code, Some(123), "{}", run.stderr);
    assert!(
        run.stderr.contains("changes not written back"),
        "{}",
        run.stderr
    );
    assert_eq!(body["result"]["exit_code"], 0);
    assert_eq!(body["result"]["write_back"]["outcome"], "not_written");
    assert_eq!(body["result"]["write_back"]["reason"], "workspace_moved");
    assert_eq!(body["result"]["changes"].as_array().unwrap().len(), 1);
    assert_eq!(world.read("note.txt"), "base\n", "nothing reached the view");
}

#[tokio::test]
async fn a_second_build_downloads_fewer_objects_and_the_binary_it_built_runs() {
    let world = World::new().await;

    let (code, first) = world.run_json("cargo build").await;
    let first_counts = objects(&first);
    eprintln!(
        "step 4: first `cargo build` exit {code:?}, objects (downloaded, from cache) {first_counts:?}"
    );
    assert_eq!(code, Some(0));
    let (code, second) = world.run_json("cargo build").await;
    let second_counts = objects(&second);
    eprintln!(
        "step 4: second `cargo build` exit {code:?}, objects (downloaded, from cache) {second_counts:?}"
    );
    assert_eq!(code, Some(0));
    assert!(first_counts.0 > 0, "the first run downloaded objects");
    assert!(
        second_counts.0 < first_counts.0,
        "{second_counts:?} is not fewer than {first_counts:?}"
    );

    let destination = world.scratch.join("demo");
    let fetched = world
        .alice
        .ch_in(
            &world.view,
            &["fetch", "target/debug/demo", destination.to_str().unwrap()],
        )
        .await;
    assert_eq!(fetched.code, Some(0), "{}", fetched.stderr);
    let mode = std::fs::metadata(&destination)
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0, "the binary is executable: {mode:o}");
    let output = std::process::Command::new(&destination).output().unwrap();
    let printed = String::from_utf8(output.stdout).unwrap();
    eprintln!(
        "step 5: `ch fetch target/debug/demo` exit {:?}; running it exit {:?} printed {printed:?}",
        fetched.code,
        output.status.code()
    );
    assert!(output.status.success());
    assert_eq!(printed, "sum 5\n");
}

#[tokio::test]
async fn refuses_a_member_without_run_with_a_403_and_exit_125() {
    let world = World::new().await;
    let (subject, issuer) = identity(&world.scratch, "carol").await;
    world
        .alice
        .ok(&[
            "member", "add", &subject, "--issuer", &issuer, "--read", "", "--write", "",
        ])
        .await;
    let carol = TokenUser::with_token(
        &world.server,
        "carol",
        &std::fs::read_to_string(world.scratch.join("token-carol")).unwrap(),
    );
    carol.ok(&["use", "acme/app"]).await;
    let view = world.scratch.join("carol-view");
    carol.ok(&["open", view.to_str().unwrap()]).await;

    let refused = carol.ch_in(&view, &["run", "--", "true"]).await;
    let direct = reqwest::Client::new()
        .post(format!(
            "{}/v1/orgs/acme/repos/app/drafts/default/runs",
            world.server.base
        ))
        .bearer_auth(carol.token())
        .json(&json!({ "command": "true", "add": [], "outputs": [] }))
        .send()
        .await
        .unwrap()
        .status();
    eprintln!(
        "step 6: carol's `ch run` exit {:?}, stderr {:?}; POST runs answers {direct}",
        refused.code, refused.stderr
    );
    assert_eq!(refused.code, Some(125), "{}", refused.stderr);
    assert_eq!(direct, StatusCode::FORBIDDEN);

    // The control: granted `run`, carol's same command runs, so the refusal was the missing grant.
    world
        .alice
        .ok(&["member", "add", &subject, "--issuer", &issuer, "--run"])
        .await;
    let allowed = carol.ch_in(&view, &["run", "--", "true"]).await;
    eprintln!(
        "step 6 control: with `run` granted, exit {:?}",
        allowed.code
    );
    assert_eq!(allowed.code, Some(0), "{}", allowed.stderr);
}
