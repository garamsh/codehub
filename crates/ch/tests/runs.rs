//! `ch run` and `ch fetch` (ADR 0051), run as the built `ch` against the HTTP API served in
//! process, with the test as the host runner that claims the run's job and reports it.

#[cfg(test)]
pub mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Change, Output, Report, Run, Server, User, put, repository, serve, unique};

/// How long one run of `ch` may take.
#[cfg(test)]
const BOUND: Duration = Duration::from_secs(60);

/// A directory of its own, removed when dropped.
#[cfg(test)]
struct Scratch(PathBuf);

#[cfg(test)]
impl Scratch {
    fn new() -> Self {
        Self(PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(unique("run")))
    }

    fn path(&self) -> &Path {
        &self.0
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

/// Runs `ch` with `arguments` in the working directory `dir`.
#[cfg(test)]
async fn at(user: &User, dir: &Path, arguments: &[&str]) -> Run {
    let mut command = user.command(arguments);
    command.current_dir(dir);
    let child = command.spawn().unwrap();
    let output = tokio::time::timeout(BOUND, child.wait_with_output())
        .await
        .unwrap_or_else(|_| panic!("ch {arguments:?} did not finish within the bound"))
        .unwrap();
    Run {
        code: output.status.code(),
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// A repository with `files` landed, a view of its draft `default` opened, and a host runner.
#[cfg(test)]
struct World {
    server: Server,
    ada: User,
    org: String,
    runner: String,
    view: Scratch,
}

#[cfg(test)]
async fn world(files: &[(&str, &str)]) -> World {
    let server = serve().await;
    let ada = server.user("ada");
    let org = repository(&ada).await;
    for (path, content) in files {
        put(&ada, "default", path, content).await;
    }
    ada.ok(&["land", "-m", "base"]).await;
    let runner = server.register_runner(&ada, "runner-a").await;
    let view = Scratch::new();
    ada.ok(&["open", view.path().to_str().unwrap()]).await;
    World {
        server,
        ada,
        org,
        runner,
        view,
    }
}

#[cfg(test)]
impl World {
    /// Runs `ch run` with `arguments` in the view while the runner reports `report` after
    /// `during` has run.
    async fn run_with(&self, arguments: &[&str], report: Report, during: impl FnOnce()) -> Run {
        let owner = self.server.token("ada");
        let runner = async {
            let claimed = self.server.claim_run(&self.runner).await;
            during();
            self.server
                .report_run(&claimed, &owner, &self.org, &report)
                .await;
        };
        let (run, ()) = tokio::join!(at(&self.ada, self.view.path(), arguments), runner);
        run
    }

    async fn run(&self, report: Report) -> Run {
        self.run_with(&["run", "--", "make"], report, || ()).await
    }
}

#[cfg(test)]
fn change(path: &str, content: &str) -> Change {
    Change::File {
        path: path.to_owned(),
        content: content.as_bytes().to_vec(),
        executable: false,
    }
}

#[tokio::test]
async fn prints_what_the_command_wrote_and_exits_with_its_code() {
    let world = world(&[("a.txt", "one\n")]).await;
    let mut report = Report::exited(0);
    report.stdout = Some("built\n".into());
    report.stderr = Some("warned\n".into());
    let run = world.run(report).await;
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(run.out(), "built\n");
    assert_eq!(run.stderr, "warned\nexit 0\n");

    let mut report = Report::exited(3);
    report.stdout = Some("partial\n".into());
    let run = world.run(report).await;
    assert_eq!(run.code, Some(3), "{}", run.stderr);
    assert_eq!(run.out(), "partial\n");
    assert!(run.stderr.ends_with("exit 3\n"), "{}", run.stderr);
}

#[tokio::test]
async fn writes_the_changes_of_a_run_into_the_view_so_the_next_snapshot_reverts_none() {
    let world = world(&[("a.txt", "one\n"), ("gone.txt", "bye\n")]).await;
    let mut report = Report::exited(0);
    report.changes = vec![
        change("a.txt", "two\n"),
        change("new/b.txt", "fresh\n"),
        Change::Deleted("gone.txt".into()),
    ];
    let run = world
        .run_with(&["run", "--add", "new/b.txt", "--", "make"], report, || ())
        .await;
    assert_eq!(run.code, Some(0), "{}", run.stderr);

    let view = &world.view;
    assert_eq!(
        std::fs::read_to_string(view.join("a.txt")).unwrap(),
        "two\n"
    );
    assert_eq!(
        std::fs::read_to_string(view.join("new/b.txt")).unwrap(),
        "fresh\n"
    );
    assert!(!view.join("gone.txt").exists());
    // The workspace holds the run's changes and the view agrees, so a snapshot reverts none.
    let status = at(&world.ada, view.path(), &["status"]).await;
    assert_eq!(status.code, Some(0), "{}", status.stderr);
    assert_eq!(
        status.out(),
        "modified a.txt\ndeleted gone.txt\nadded new/b.txt\n"
    );
}

#[tokio::test]
async fn leaves_a_file_edited_during_the_run_and_names_it() {
    let world = world(&[("a.txt", "one\n"), ("b.txt", "keep\n")]).await;
    let mut report = Report::exited(0);
    report.changes = vec![change("a.txt", "from run\n"), change("b.txt", "from run\n")];
    let edited = world.view.join("a.txt");
    let run = world
        .run_with(&["run", "--", "make"], report, || {
            std::fs::write(&edited, "mine\n").unwrap();
        })
        .await;
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        std::fs::read_to_string(world.view.join("a.txt")).unwrap(),
        "mine\n"
    );
    assert_eq!(
        std::fs::read_to_string(world.view.join("b.txt")).unwrap(),
        "from run\n"
    );
    assert!(run.stderr.contains("a.txt"), "{}", run.stderr);
    assert!(!run.stderr.contains("b.txt was edited"), "{}", run.stderr);
}

#[tokio::test]
async fn exits_123_and_leaves_the_view_when_the_workspace_moved_during_the_run() {
    let world = world(&[("a.txt", "one\n")]).await;
    let mut report = Report::exited(0);
    report.changes = vec![change("a.txt", "from run\n")];
    let outside = Scratch::new();
    std::fs::create_dir_all(outside.path()).unwrap();
    let ada = &world.ada;
    let source = common::file(b"elsewhere\n");
    let run = {
        let owner = world.server.token("ada");
        let runner = async {
            let claimed = world.server.claim_run(&world.runner).await;
            ada.ok(&["put", "moved.txt", &source]).await;
            world
                .server
                .report_run(&claimed, &owner, &world.org, &report)
                .await;
        };
        let (run, ()) = tokio::join!(at(ada, world.view.path(), &["run", "--", "make"]), runner);
        run
    };
    assert_eq!(run.code, Some(123), "{}", run.stderr);
    assert!(run.stderr.contains("not written back"), "{}", run.stderr);
    assert_eq!(
        std::fs::read_to_string(world.view.join("a.txt")).unwrap(),
        "one\n"
    );
}

#[tokio::test]
async fn prints_the_streams_of_a_run_that_timed_out_and_exits_124() {
    let world = world(&[("a.txt", "one\n")]).await;
    let mut report = Report::stopped("timed_out");
    report.stdout = Some("so far\n".into());
    let run = world.run(report).await;
    assert_eq!(run.code, Some(124), "{}", run.stderr);
    assert_eq!(run.out(), "so far\n");
    assert!(run.stderr.contains("timed out"), "{}", run.stderr);
}

#[tokio::test]
async fn exits_126_when_the_runner_could_not_run_the_command() {
    let world = world(&[("a.txt", "one\n")]).await;
    let run = world.run(Report::stopped("executor_failed")).await;
    assert_eq!(run.code, Some(126), "{}", run.stderr);
}

#[tokio::test]
async fn exits_125_when_the_run_was_not_started() {
    let server = serve().await;
    let ada = server.user("ada");
    let org = repository(&ada).await;
    let bob = server.user("bob");
    bob.ch(&["use", &format!("{org}/app")]).await;
    let run = bob.ch(&["run", "--", "make"]).await;
    assert_eq!(run.code, Some(125), "{}", run.stderr);
    assert!(run.stderr.starts_with("ch: "), "{}", run.stderr);
}

#[tokio::test]
async fn prints_the_run_as_json_without_download_grants() {
    let world = world(&[("a.txt", "one\n")]).await;
    let mut report = Report::exited(0);
    report.stdout = Some("built\n".into());
    let run = world
        .run_with(&["run", "--json", "--", "make"], report, || ())
        .await;
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    let value: serde_json::Value = serde_json::from_str(&run.out()).unwrap();
    assert_eq!(value["result"]["outcome"], "exited");
    assert_eq!(value["result"]["exit_code"], 0);
    assert!(value["result"]["stdout"]["address"].is_string());
    assert!(
        value["result"]["stdout"].get("download").is_none(),
        "{value}"
    );
}

#[tokio::test]
async fn drops_a_written_back_path_the_view_ignores_from_the_workspace_at_the_next_snapshot() {
    let world = world(&[("a.txt", "one\n"), (".codehub/ignore", "gen/\n")]).await;
    // The control: the ignore file reached the view.
    assert_eq!(
        std::fs::read_to_string(world.view.join(".codehub/ignore")).unwrap(),
        "gen/\n"
    );
    let mut report = Report::exited(0);
    report.changes = vec![change("gen/x", "made\n")];
    let run = world
        .run_with(&["run", "--add", "gen/x", "--", "make"], report, || ())
        .await;
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(
        std::fs::read_to_string(world.view.join("gen/x")).unwrap(),
        "made\n"
    );
    let ada = world.server.token("ada");
    let version = world
        .server
        .workspace_version(&ada, &world.org, "default")
        .await;
    // The view ignores the path and the base lacks it, so the next snapshot drops the add from
    // the workspace; the file stays in the view.
    let status = at(&world.ada, world.view.path(), &["status"]).await;
    assert_eq!(status.out(), "", "{}", status.stderr);
    let workspace = world
        .server
        .workspace_json(&ada, &world.org, "default")
        .await;
    assert!(workspace.contains(r#""changes":[]"#), "{workspace}");
    assert!(world.view.join("gen/x").exists());
    assert!(
        world
            .server
            .workspace_version(&ada, &world.org, "default")
            .await
            > version
    );
    // Deleting it afterwards is then a view edit that sends nothing.
    std::fs::remove_file(world.view.join("gen/x")).unwrap();
    let version = world
        .server
        .workspace_version(&ada, &world.org, "default")
        .await;
    let status = at(&world.ada, world.view.path(), &["status"]).await;
    assert_eq!(status.out(), "", "{}", status.stderr);
    assert_eq!(
        world
            .server
            .workspace_version(&ada, &world.org, "default")
            .await,
        version
    );
}

#[tokio::test]
async fn fetches_a_file_and_a_directory_of_the_latest_run_keeping_the_executable_bit() {
    let world = world(&[("a.txt", "one\n")]).await;
    let mut report = Report::exited(0);
    report.outputs = vec![
        Output {
            path: "dist/app".into(),
            content: b"#!/bin/sh\n".to_vec(),
            executable: true,
        },
        Output {
            path: "dist/doc/readme.txt".into(),
            content: b"read me\n".to_vec(),
            executable: false,
        },
    ];
    let run = world
        .run_with(&["run", "--output", "dist", "--", "make"], report, || ())
        .await;
    assert_eq!(run.code, Some(0), "{}", run.stderr);

    let out = Scratch::new();
    std::fs::create_dir_all(out.path()).unwrap();
    let fetched = at(
        &world.ada,
        world.view.path(),
        &["fetch", "dist/app", out.join("app").to_str().unwrap()],
    )
    .await;
    assert_eq!(fetched.code, Some(0), "{}", fetched.stderr);
    let mode = std::fs::metadata(out.join("app"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111);

    let fetched = at(
        &world.ada,
        world.view.path(),
        &["fetch", "dist", out.join("tree").to_str().unwrap()],
    )
    .await;
    assert_eq!(fetched.code, Some(0), "{}", fetched.stderr);
    assert_eq!(
        std::fs::read_to_string(out.join("tree/doc/readme.txt")).unwrap(),
        "read me\n"
    );
    let mode = std::fs::metadata(out.join("tree/doc/readme.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0);

    let missing = at(
        &world.ada,
        world.view.path(),
        &["fetch", "nothing", out.join("x").to_str().unwrap()],
    )
    .await;
    assert_ne!(missing.code, Some(0));
    assert!(!out.join("x").exists());
}

#[tokio::test]
async fn cancels_the_run_on_an_interrupt_and_exits_124() {
    let world = world(&[("a.txt", "one\n")]).await;
    let mut command = world.ada.command(&["run", "--", "make"]);
    command.current_dir(world.view.path());
    let child = command.spawn().unwrap();
    let claimed = world.server.claim_run(&world.runner).await;
    let pid = child.id().unwrap().to_string();
    // Let `ch` reach its wait before the interrupt, which would otherwise end it unhandled.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let signalled = std::process::Command::new("kill")
        .args(["-INT", &pid])
        .status()
        .unwrap();
    assert!(signalled.success());
    let output = tokio::time::timeout(BOUND, child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(124), "{stderr}");
    assert!(stderr.contains("cancelled"), "{stderr}");
    let _ = claimed;
}
