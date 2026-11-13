//! `ch checks`, run as the built binary against the HTTP API served in process, with the runner's
//! side of the job conversation played over HTTP.

#[cfg(test)]
pub mod common;

use std::time::Duration;

use app::domain::address::Address;
use common::{Server, User, put, repository, serve};
use serde_json::{Value, json};

/// Flags that make `--wait` poll fast and give up soon.
#[cfg(test)]
const QUICK: [&str; 4] = ["--poll-interval-ms", "50", "--timeout", "2"];

/// A server with the repository `<org>/app` of `ada`, its first landing, whose tree defines
/// `checks` and so queues them, and a runner `runner` registered in the organisation. Returns
/// the organisation, the changeset and the runner's token.
#[cfg(test)]
async fn landed(server: &Server, ada: &User, checks: &[(&str, bool)]) -> (String, String, String) {
    let org = repository(ada).await;
    put(ada, "default", "a.txt", "one\n").await;
    if !checks.is_empty() {
        put(
            ada,
            "default",
            ".codehub/checks.toml",
            &server.checks_file(checks),
        )
        .await;
    }
    let changeset = ada.ok(&["land", "-m", "one"]).await.trim().to_owned();
    ada.ok(&["runner", "register", "runner", "--issuer", Server::ISSUER])
        .await;
    (org, changeset, server.token("runner"))
}

#[tokio::test]
async fn shows_last_green_none_and_a_line_per_job_of_the_head_of_main() {
    let server = serve().await;
    let ada = server.user("ada");
    let (_, changeset, _) = landed(&server, &ada, &[("build", true), ("lint", false)]).await;

    let expected = "last-green none\nbuild blocking queued -\nlint non-blocking queued -\n";
    assert_eq!(ada.ok(&["checks"]).await, expected);
    // The control: naming the changeset shows the same, so the default was the head of main.
    assert_eq!(ada.ok(&["checks", &changeset]).await, expected);
}

#[tokio::test]
async fn shows_the_state_and_exit_code_of_each_job_and_where_last_green_stands() {
    let server = serve().await;
    let ada = server.user("ada");
    let (_, changeset, runner) = landed(&server, &ada, &[("build", true), ("lint", false)]).await;

    server.finish_job(&runner, "passed", Some(0)).await;
    assert_eq!(
        ada.ok(&["checks"]).await,
        format!("last-green {changeset}\nbuild blocking passed 0\nlint non-blocking queued -\n")
    );

    // A failing non-blocking check reverts nothing and leaves last-green where it was.
    server.finish_job(&runner, "failed", Some(7)).await;
    assert_eq!(
        ada.ok(&["checks"]).await,
        format!("last-green {changeset}\nbuild blocking passed 0\nlint non-blocking failed 7\n")
    );
}

#[tokio::test]
async fn names_the_changeset_that_reverts_a_failed_blocking_check() {
    let server = serve().await;
    let ada = server.user("ada");
    let (_, changeset, runner) = landed(&server, &ada, &[("build", true)]).await;
    server.finish_job(&runner, "failed", Some(3)).await;

    let shown = ada.ok(&["checks", &changeset]).await;
    let lines: Vec<&str> = shown.lines().collect();
    assert_eq!(lines[1], "build blocking failed 3");
    let revert = lines[2].strip_prefix("revert ").unwrap();
    assert!(revert.parse::<Address>().is_ok(), "{shown}");
    assert_ne!(revert, changeset);
    assert_eq!(lines.len(), 3, "{shown}");
}

#[tokio::test]
async fn names_the_paths_of_a_revert_that_conflicts() {
    let server = serve().await;
    let ada = server.user("ada");
    let (_, changeset, runner) = landed(&server, &ada, &[("build", true)]).await;
    put(&ada, "default", "a.txt", "two\n").await;
    ada.ok(&["land", "-m", "two"]).await;
    server.finish_job(&runner, "failed", Some(1)).await;

    let shown = ada.ok(&["checks", &changeset]).await;
    assert_eq!(
        shown,
        "last-green none\nbuild blocking failed 1\nconflict a.txt\n"
    );
}

#[tokio::test]
async fn shows_the_reason_of_a_revert_that_failed() {
    let server = serve().await;
    let ada = server.user("ada");
    let (org, changeset, runner) = landed(&server, &ada, &[("build", true)]).await;
    server.finish_job(&runner, "failed", Some(3)).await;
    server
        .record_failed_revert(&org, &changeset, "revert not landed: log down")
        .await;

    let shown = ada.ok(&["checks", &changeset]).await;
    let lines: Vec<&str> = shown.lines().collect();
    assert_eq!(
        lines[1..],
        [
            "build blocking failed 3",
            "revert failed: revert not landed: log down"
        ]
    );
    let json: Value =
        serde_json::from_str(&ada.ok(&["--json", "checks", &changeset]).await).unwrap();
    assert_eq!(
        json["checks"][0]["revert"],
        json!({"outcome": "failed", "reason": "revert not landed: log down"})
    );
}

#[tokio::test]
async fn prints_the_checks_as_json() {
    let server = serve().await;
    let ada = server.user("ada");
    let (_, changeset, runner) = landed(&server, &ada, &[("build", true)]).await;
    server.finish_job(&runner, "passed", Some(0)).await;

    let shown: Value = serde_json::from_str(&ada.ok(&["--json", "checks"]).await).unwrap();
    assert_eq!(shown["last_green"], json!(changeset));
    let checks = shown["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0]["name"], "build");
    assert_eq!(checks[0]["state"], "passed");
    assert_eq!(checks[0]["exit_code"], 0);
}

#[tokio::test]
async fn exits_5_for_a_repository_the_caller_may_not_see() {
    let server = serve().await;
    let ada = server.user("ada");
    let bob = server.user("bob");
    let (org, changeset, _) = landed(&server, &ada, &[]).await;
    bob.ok(&["use", &format!("{org}/app")]).await;

    let hidden = bob.ch(&["checks", &changeset]).await;
    assert_eq!(hidden.code, Some(5), "{}", hidden.stderr);
    assert!(hidden.stdout.is_empty());
    // The control: a member who may read it sees the same changeset.
    ada.ok(&["member", "add", "bob", "--read", ""]).await;
    bob.ok(&["checks", &changeset]).await;
}

#[tokio::test]
async fn waits_and_exits_0_when_every_blocking_check_passed() {
    let server = serve().await;
    let ada = server.user("ada");
    let (_, _, runner) = landed(&server, &ada, &[("build", true), ("lint", false)]).await;

    let mut args = vec!["checks", "--wait"];
    args.extend(QUICK);
    let mut waiting = ada.command(&args).spawn().unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    // The waiter has not stopped while the checks are queued.
    assert!(waiting.try_wait().unwrap().is_none());
    server.finish_job(&runner, "passed", Some(0)).await;
    // A non-blocking check that fails does not count.
    server.finish_job(&runner, "failed", Some(9)).await;

    let output = waiting.wait_with_output().await.unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("build blocking passed 0"), "{text}");
    assert!(text.contains("lint non-blocking failed 9"), "{text}");
}

#[tokio::test]
async fn waits_and_exits_4_when_a_blocking_check_failed_or_timed_out() {
    let server = serve().await;
    let ada = server.user("ada");
    let (_, changeset, runner) = landed(&server, &ada, &[("build", true), ("lint", true)]).await;
    server.finish_job(&runner, "failed", Some(2)).await;
    server.finish_job(&runner, "timed_out", None).await;

    let mut args = vec!["checks", &changeset, "--wait"];
    args.extend(QUICK);
    let run = ada.ch(&args).await;
    assert_eq!(run.code, Some(4), "{}", run.stderr);
    assert!(
        run.out().contains("build blocking failed 2"),
        "{}",
        run.out()
    );
    assert!(
        run.out().contains("lint blocking timed_out -"),
        "{}",
        run.out()
    );
    assert!(run.stderr.contains("build, lint"), "{}", run.stderr);

    // The control: without --wait the same state is only shown.
    ada.ok(&["checks", &changeset]).await;
}

#[tokio::test]
async fn waits_and_exits_4_naming_the_jobs_that_had_not_ended_at_the_timeout() {
    let server = serve().await;
    let ada = server.user("ada");
    let (_, changeset, runner) = landed(&server, &ada, &[("build", true), ("lint", true)]).await;
    server.finish_job(&runner, "passed", Some(0)).await;

    let run = ada
        .ch(&[
            "checks",
            &changeset,
            "--wait",
            "--poll-interval-ms",
            "50",
            "--timeout",
            "1",
        ])
        .await;
    assert_eq!(run.code, Some(4), "{}", run.stderr);
    assert!(run.stderr.contains("timed out"), "{}", run.stderr);
    assert!(run.stderr.contains("not ended: lint"), "{}", run.stderr);
    assert!(!run.stderr.contains("build"), "{}", run.stderr);
    assert!(
        run.out().contains("lint blocking queued -"),
        "{}",
        run.out()
    );
}
