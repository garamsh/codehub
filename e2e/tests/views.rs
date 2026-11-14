//! A view through the built `ch` against the built `codehub`: `open`, an edit, `status`, `land`,
//! and then `read` and `log`.

#[cfg(test)]
pub mod common;

use std::path::Path;
use std::time::Duration;

use common::{Server, TestDatabase, User, unique};

/// How long one run of `ch` may take.
#[cfg(test)]
const BOUND: Duration = Duration::from_secs(60);

/// Runs `ch` with `arguments` in the directory `dir`, expects exit 0 and returns its stdout.
#[cfg(test)]
async fn ok_at(user: &User, dir: &Path, arguments: &[&str]) -> String {
    let mut command = user.command(arguments);
    command.current_dir(dir);
    let child = command.spawn().unwrap();
    let output = tokio::time::timeout(BOUND, child.wait_with_output())
        .await
        .unwrap_or_else(|_| panic!("ch {arguments:?} did not finish within the bound"))
        .unwrap();
    assert!(
        output.status.success(),
        "ch {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test]
async fn lands_an_edited_view_and_serves_it_from_read_and_log() {
    let database = TestDatabase::new().await;
    let server = Server::start(&database);
    let alice = User::new("alice");
    alice.login(&server).await;
    alice.create_repository(&unique("org"), "app").await;
    alice.stage("default", "src/a.txt", "one\n").await;
    alice.ok(&["land", "-m", "base"]).await;

    let view = alice.home().join(unique("view"));
    alice.ok(&["open", view.to_str().unwrap()]).await;
    assert_eq!(
        std::fs::read_to_string(view.join("src/a.txt")).unwrap(),
        "one\n"
    );

    std::fs::write(view.join("src/a.txt"), "two\n").unwrap();
    assert_eq!(
        ok_at(&alice, &view, &["status"]).await,
        "modified src/a.txt\n"
    );
    ok_at(&alice, &view, &["describe", "edit in a view"]).await;
    let landed = ok_at(&alice, &view, &["land"]).await.trim().to_owned();

    assert_eq!(alice.ok(&["read", "src/a.txt"]).await, "two\n");
    let log = alice.ok(&["log", "src/a.txt"]).await;
    let newest = log.lines().next().unwrap();
    assert!(newest.starts_with(&landed), "{log}");
    assert!(newest.ends_with(" edit in a view"), "{log}");
    assert_eq!(log.lines().count(), 2, "{log}");
}
