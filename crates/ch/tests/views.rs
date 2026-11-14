//! The views of ADR 0048, run as the built `ch` against the HTTP API served in process: a view is
//! opened into a directory, edited with the file system, and landed.

#[cfg(test)]
pub mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::{Run, User, put, repository, serve, unique};

/// How long one run of `ch` may take.
#[cfg(test)]
const BOUND: Duration = Duration::from_secs(60);

/// A directory of its own, removed when dropped.
#[cfg(test)]
struct Scratch(PathBuf);

#[cfg(test)]
impl Scratch {
    fn new() -> Self {
        Self(PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(unique("view")))
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

/// Runs `ch` in `dir`, expects exit 0 and returns its stdout.
#[cfg(test)]
async fn ok_at(user: &User, dir: &Path, arguments: &[&str]) -> String {
    let run = at(user, dir, arguments).await;
    assert_eq!(run.code, Some(0), "ch {arguments:?}: {}", run.stderr);
    run.out()
}

/// Opens the draft `default` into `view`.
#[cfg(test)]
async fn open(user: &User, view: &Scratch) {
    user.ok(&["open", view.path().to_str().unwrap()]).await;
}

/// Stages `files` in `default` and lands them.
#[cfg(test)]
async fn base(user: &User, files: &[(&str, &str)]) {
    for (path, content) in files {
        put(user, "default", path, content).await;
    }
    user.ok(&["land", "-m", "base"]).await;
}

#[cfg(test)]
fn read(view: &Scratch, path: &str) -> String {
    std::fs::read_to_string(view.join(path)).unwrap()
}

#[tokio::test]
async fn edits_a_view_with_the_file_system_and_lands_it() {
    let server = serve().await;
    let ada = server.user("ada");
    let org = repository(&ada).await;
    base(
        &ada,
        &[
            ("a.txt", "one\ntwo\n"),
            ("b.txt", "keep\n"),
            ("c.txt", "gone\n"),
        ],
    )
    .await;

    let view = Scratch::new();
    open(&ada, &view).await;
    assert_eq!(read(&view, "a.txt"), "one\ntwo\n");
    assert_eq!(read(&view, "b.txt"), "keep\n");
    // The control: nothing is changed before an edit.
    assert_eq!(ok_at(&ada, view.path(), &["status"]).await, "");

    std::fs::write(view.join("a.txt"), "one\nthree\n").unwrap();
    std::fs::write(view.join("new.txt"), "fresh\n").unwrap();
    std::fs::remove_file(view.join("c.txt")).unwrap();

    let status = ok_at(&ada, view.path(), &["status"]).await;
    assert_eq!(status, "modified a.txt\ndeleted c.txt\nadded new.txt\n");
    let diff = ok_at(&ada, view.path(), &["diff", "a.txt"]).await;
    assert!(diff.contains("-two"), "{diff}");
    assert!(diff.contains("+three"), "{diff}");
    assert!(!diff.contains("new.txt"), "{diff}");

    ok_at(&ada, view.path(), &["describe", "my change"]).await;
    let landed = ok_at(&ada, view.path(), &["land"]).await.trim().to_owned();

    assert_eq!(ada.ok(&["read", "a.txt"]).await, "one\nthree\n");
    assert_eq!(ada.ok(&["read", "b.txt"]).await, "keep\n");
    assert_eq!(ada.ok(&["read", "new.txt"]).await, "fresh\n");
    let gone = ada.ch(&["read", "c.txt"]).await;
    assert_eq!(gone.code, Some(5), "{}", gone.stderr);
    assert_eq!(server.changeset_message(&org, &landed).await, "my change");
}

#[tokio::test]
async fn refuses_put_inside_a_view_with_exit_2_and_puts_outside_it() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    let view = Scratch::new();
    open(&ada, &view).await;
    let source = common::file(b"via put\n");

    for arguments in [
        vec!["put", "p.txt", source.as_str()],
        vec!["put", "--delete", "p.txt"],
    ] {
        let refused = at(&ada, view.path(), &arguments).await;
        assert_eq!(refused.code, Some(2), "{}", refused.stderr);
        assert!(
            refused.stderr.contains(view.path().to_str().unwrap()),
            "{}",
            refused.stderr
        );
    }
    // The refusals wrote nothing to the workspace.
    assert_eq!(ok_at(&ada, view.path(), &["status"]).await, "");

    // The control: the same put outside the view succeeds and lands.
    let outside = Scratch::new();
    std::fs::create_dir(outside.path()).unwrap();
    ok_at(&ada, outside.path(), &["put", "p.txt", source.as_str()]).await;
    ok_at(&ada, outside.path(), &["land", "-m", "outside"]).await;
    assert_eq!(ada.ok(&["read", "p.txt"]).await, "via put\n");
}

#[tokio::test]
async fn lands_with_the_message_given_in_place_of_the_description() {
    let server = serve().await;
    let ada = server.user("ada");
    let org = repository(&ada).await;
    let view = Scratch::new();
    open(&ada, &view).await;
    std::fs::write(view.join("a.txt"), "one\n").unwrap();
    ok_at(&ada, view.path(), &["describe", "described"]).await;
    let landed = ok_at(&ada, view.path(), &["land", "--message", "given"]).await;

    assert_eq!(server.changeset_message(&org, landed.trim()).await, "given");
    assert_eq!(ada.ok(&["read", "a.txt"]).await, "one\n");
}

#[tokio::test]
async fn reopens_the_draft_in_a_new_directory_after_the_view_is_lost() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    base(&ada, &[("a.txt", "one\n"), ("c.txt", "gone\n")]).await;

    let view = Scratch::new();
    open(&ada, &view).await;
    std::fs::write(view.join("a.txt"), "two\n").unwrap();
    std::fs::write(view.join("new.txt"), "fresh\n").unwrap();
    std::fs::remove_file(view.join("c.txt")).unwrap();
    ok_at(&ada, view.path(), &["describe", "kept"]).await;
    let before = ok_at(&ada, view.path(), &["status"]).await;
    drop(view);

    let again = Scratch::new();
    open(&ada, &again).await;
    assert_eq!(read(&again, "a.txt"), "two\n");
    assert_eq!(read(&again, "new.txt"), "fresh\n");
    assert!(!again.join("c.txt").exists());
    assert_eq!(ok_at(&ada, again.path(), &["status"]).await, before);
    let json = ok_at(&ada, again.path(), &["--json", "status"]).await;
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json["description"], "kept");
}

#[tokio::test]
async fn keeps_ignored_paths_out_unless_the_base_holds_them() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    base(
        &ada,
        &[
            (".codehub/ignore", "*.log\n"),
            ("kept.log", "one\n"),
            ("a.txt", "one\n"),
        ],
    )
    .await;

    let view = Scratch::new();
    open(&ada, &view).await;
    std::fs::write(view.join("new.log"), "ignored\n").unwrap();
    std::fs::write(view.join("kept.log"), "two\n").unwrap();
    std::fs::write(view.join("new.txt"), "fresh\n").unwrap();

    let status = ok_at(&ada, view.path(), &["status"]).await;
    // The control: a path the rules do not match is recorded; the base's own is still tracked.
    assert_eq!(status, "modified kept.log\nadded new.txt\n");
    assert!(!status.contains("new.log"), "{status}");
    // `.ch/` is never snapshotted.
    assert!(!status.contains(".ch"), "{status}");
}

#[tokio::test]
async fn restores_a_path_to_the_base_in_the_view_and_in_the_status() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    base(&ada, &[("a.txt", "one\n"), ("c.txt", "gone\n")]).await;

    let view = Scratch::new();
    open(&ada, &view).await;
    std::fs::write(view.join("a.txt"), "two\n").unwrap();
    std::fs::write(view.join("new.txt"), "fresh\n").unwrap();
    std::fs::remove_file(view.join("c.txt")).unwrap();
    let status = ok_at(&ada, view.path(), &["status"]).await;
    assert_eq!(status, "modified a.txt\ndeleted c.txt\nadded new.txt\n");

    ok_at(&ada, view.path(), &["restore", "a.txt"]).await;
    assert_eq!(read(&view, "a.txt"), "one\n");
    let status = ok_at(&ada, view.path(), &["status"]).await;
    assert_eq!(status, "deleted c.txt\nadded new.txt\n");

    ok_at(&ada, view.path(), &["restore", "c.txt"]).await;
    assert_eq!(read(&view, "c.txt"), "gone\n");
    ok_at(&ada, view.path(), &["restore", "new.txt"]).await;
    assert!(!view.join("new.txt").exists());
    assert_eq!(ok_at(&ada, view.path(), &["status"]).await, "");
}

#[tokio::test]
async fn returns_a_file_edited_back_to_its_base_content_as_unchanged() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    base(&ada, &[("a.txt", "one\n")]).await;

    let view = Scratch::new();
    open(&ada, &view).await;
    std::fs::write(view.join("a.txt"), "two\n").unwrap();
    assert_eq!(
        ok_at(&ada, view.path(), &["status"]).await,
        "modified a.txt\n"
    );
    std::fs::write(view.join("a.txt"), "one\n").unwrap();
    assert_eq!(ok_at(&ada, view.path(), &["status"]).await, "");
}

#[tokio::test]
async fn refuses_a_view_verb_after_land_and_outside_a_view() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    let view = Scratch::new();
    open(&ada, &view).await;
    std::fs::write(view.join("a.txt"), "one\n").unwrap();

    // The control: every verb runs while the view is open.
    for arguments in [
        &["status"][..],
        &["diff"],
        &["describe", "text"],
        &["restore", "missing.txt"],
    ] {
        let run = at(&ada, view.path(), arguments).await;
        assert_eq!(run.code, Some(0), "{arguments:?}: {}", run.stderr);
    }
    ok_at(&ada, view.path(), &["land"]).await;

    for arguments in [
        &["status"][..],
        &["diff"],
        &["describe", "text"],
        &["restore", "a.txt"],
        &["land"],
    ] {
        let run = at(&ada, view.path(), arguments).await;
        assert_eq!(run.code, Some(2), "{arguments:?}: {}", run.stderr);
        assert!(run.stderr.contains("ch open"), "{}", run.stderr);
        assert!(run.stdout.is_empty());
    }

    let outside = Scratch::new();
    std::fs::create_dir_all(outside.path()).unwrap();
    let run = at(&ada, outside.path(), &["status"]).await;
    assert_eq!(run.code, Some(2), "{}", run.stderr);
    assert!(run.stderr.contains("ch open"), "{}", run.stderr);
}

#[tokio::test]
async fn needs_a_message_to_land_outside_a_view() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    put(&ada, "default", "a.txt", "one\n").await;
    let run = ada.ch(&["land"]).await;
    assert_eq!(run.code, Some(2), "{}", run.stderr);
    // The control: with a message it lands.
    ada.ok(&["land", "-m", "text"]).await;
}

#[tokio::test]
async fn opens_only_into_an_empty_directory() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    let view = Scratch::new();
    std::fs::create_dir_all(view.path()).unwrap();
    std::fs::write(view.join("other.txt"), "mine\n").unwrap();
    let run = ada.ch(&["open", view.path().to_str().unwrap()]).await;
    assert_eq!(run.code, Some(2), "{}", run.stderr);
    assert_eq!(read(&view, "other.txt"), "mine\n");
    assert!(!view.join(".ch").exists());
}

#[tokio::test]
async fn holds_only_the_prefix_and_leaves_the_rest_of_the_repository_alone() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    base(&ada, &[("src/a.txt", "one\n"), ("docs/b.txt", "doc\n")]).await;

    let view = Scratch::new();
    ok_at(
        &ada,
        view.path().parent().unwrap(),
        &["open", view.path().to_str().unwrap(), "--prefix", "src"],
    )
    .await;
    assert_eq!(read(&view, "src/a.txt"), "one\n");
    assert!(!view.join("docs").exists());

    std::fs::write(view.join("src/a.txt"), "two\n").unwrap();
    std::fs::write(view.join("src/new.txt"), "fresh\n").unwrap();
    let status = ok_at(&ada, view.path(), &["status"]).await;
    assert_eq!(status, "modified src/a.txt\nadded src/new.txt\n");
    ok_at(&ada, view.path(), &["land", "-m", "src only"]).await;

    assert_eq!(ada.ok(&["read", "src/a.txt"]).await, "two\n");
    assert_eq!(ada.ok(&["read", "src/new.txt"]).await, "fresh\n");
    assert_eq!(ada.ok(&["read", "docs/b.txt"]).await, "doc\n");
}

#[tokio::test]
async fn opens_a_repository_without_a_base_and_keeps_the_executable_kind() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    let view = Scratch::new();
    open(&ada, &view).await;
    std::fs::write(view.join("run.sh"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(view.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        ok_at(&ada, view.path(), &["status"]).await,
        "added run.sh\n"
    );
    ok_at(&ada, view.path(), &["land", "-m", "script"]).await;

    let other = Scratch::new();
    open(&ada, &other).await;
    assert_eq!(read(&other, "run.sh"), "#!/bin/sh\n");
    let mode = std::fs::metadata(other.join("run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111);
}

#[tokio::test]
async fn drops_a_recorded_path_a_rule_comes_to_ignore_from_the_workspace() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    base(&ada, &[("a.txt", "one\n")]).await;

    let view = Scratch::new();
    open(&ada, &view).await;
    std::fs::create_dir_all(view.join("out")).unwrap();
    std::fs::write(view.join("out/x"), "built\n").unwrap();
    assert_eq!(ok_at(&ada, view.path(), &["status"]).await, "added out/x\n");

    std::fs::create_dir_all(view.join(".codehub")).unwrap();
    std::fs::write(view.join(".codehub/ignore"), "out/\n").unwrap();
    std::fs::remove_dir_all(view.join("out")).unwrap();
    let status = ok_at(&ada, view.path(), &["status"]).await;
    assert_eq!(status, "added .codehub/ignore\n");

    ok_at(&ada, view.path(), &["land", "-m", "ignore out"]).await;
    let gone = ada.ch(&["read", "out/x"]).await;
    assert_eq!(gone.code, Some(5), "{}", gone.stderr);
    // The control: what the rules do not ignore landed.
    assert_eq!(ada.ok(&["read", ".codehub/ignore"]).await, "out/\n");
}

#[tokio::test]
async fn refuses_to_open_a_view_inside_a_view_and_scans_around_one() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    base(&ada, &[("a.txt", "one\n")]).await;

    let view = Scratch::new();
    open(&ada, &view).await;
    let inner = view.join("sub");
    let run = ada.ch(&["open", inner.to_str().unwrap()]).await;
    assert_eq!(run.code, Some(2), "{}", run.stderr);
    assert!(
        run.stderr.contains(view.path().to_str().unwrap()),
        "{}",
        run.stderr
    );
    assert!(!inner.exists());

    // A directory that holds a view of its own is not part of this one.
    std::fs::create_dir_all(view.join("nested/.ch")).unwrap();
    std::fs::write(view.join("nested/.ch/view.json"), "{}").unwrap();
    std::fs::write(view.join("nested/x.txt"), "other\n").unwrap();
    std::fs::write(view.join("new.txt"), "fresh\n").unwrap();
    assert_eq!(
        ok_at(&ada, view.path(), &["status"]).await,
        "added new.txt\n"
    );
}

#[tokio::test]
async fn keeps_an_edit_a_failed_restore_could_not_write_over() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    base(&ada, &[("a.txt", "one\n")]).await;

    let view = Scratch::new();
    open(&ada, &view).await;
    std::fs::write(view.join("a.txt"), "two\n").unwrap();
    std::fs::set_permissions(view.join("a.txt"), std::fs::Permissions::from_mode(0o444)).unwrap();
    let run = at(&ada, view.path(), &["restore", "a.txt"]).await;
    assert_ne!(run.code, Some(0), "{}", run.stderr);
    assert_eq!(read(&view, "a.txt"), "two\n");

    // The edit is still the view's, so the next snapshot sends it again.
    assert_eq!(
        ok_at(&ada, view.path(), &["status"]).await,
        "modified a.txt\n"
    );
    ok_at(&ada, view.path(), &["land", "-m", "kept"]).await;
    assert_eq!(ada.ok(&["read", "a.txt"]).await, "two\n");
}

#[tokio::test]
async fn snapshots_on_land_and_on_diff_without_another_verb_first() {
    let server = serve().await;
    let ada = server.user("ada");
    repository(&ada).await;
    base(&ada, &[("a.txt", "one\n")]).await;

    let view = Scratch::new();
    open(&ada, &view).await;
    std::fs::write(view.join("a.txt"), "two\n").unwrap();
    let diff = ok_at(&ada, view.path(), &["diff"]).await;
    assert!(diff.contains("+two"), "{diff}");

    std::fs::write(view.join("a.txt"), "three\n").unwrap();
    std::fs::write(view.join("new.txt"), "fresh\n").unwrap();
    ok_at(&ada, view.path(), &["land", "-m", "straight"]).await;
    assert_eq!(ada.ok(&["read", "a.txt"]).await, "three\n");
    assert_eq!(ada.ok(&["read", "new.txt"]).await, "fresh\n");
}
