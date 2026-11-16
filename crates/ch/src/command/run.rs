//! `run` and `fetch` (ADR 0051): a command run on a runner against the workspace, whose changes
//! are written back into the view, and the output files of a run.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use super::workspace::{download, snapshot};
use super::{Context, Session, address, print_json};
use crate::api::Api;
use crate::cli::{Fetch, Run as RunArguments};
use crate::failure::{
    EXIT_RUN_CONFLICT, EXIT_RUN_LOST, EXIT_RUN_NOT_STARTED, EXIT_RUN_STOPPED,
    EXIT_RUN_WRITE_DENIED, Failure,
};
use crate::state::RepositoryRef;
use crate::view::{Entry, View, write_file};
use crate::wire::{
    Kind, NotWrittenReason, Run, RunChange, RunChangeKind, RunOutcome, RunResult, RunState,
    RunStreamRead, StartRun, WriteBackOutcome,
};

/// How long one read of a run is held for it to end, in seconds.
const WAIT: u64 = 60;

/// The draft a verb with no `--draft` acts on outside a view.
const DEFAULT_DRAFT: &str = "default";

/// A failure before the command could run.
fn not_started(failure: &Failure) -> Failure {
    Failure::Run {
        code: EXIT_RUN_NOT_STARTED,
        message: Some(failure.to_string()),
    }
}

/// A failure after the command started.
fn lost(message: String) -> Failure {
    Failure::Run {
        code: EXIT_RUN_LOST,
        message: Some(message),
    }
}

/// What `run` is to act on, once it has snapshotted the view.
struct Target {
    session: Session,
    view: Option<View>,
    repository: RepositoryRef,
    draft: String,
}

/// The view the working directory is in, snapshotted, the session, and the draft the run is of.
async fn target(context: &Context, arguments: &RunArguments) -> Result<Target, Failure> {
    let mut view = View::find(&std::env::current_dir()?)?
        .map(View::ensure_open)
        .transpose()?;
    let session = Session::open(context).await?;
    if let Some(view) = &mut view {
        snapshot(context, &session.api(context), view).await?;
    }
    let repository = match &view {
        Some(view) => view.repository.clone(),
        None => session.repository()?.clone(),
    };
    let draft = arguments
        .draft
        .clone()
        .or_else(|| view.as_ref().map(|view| view.draft.clone()))
        .unwrap_or_else(|| DEFAULT_DRAFT.to_owned());
    Ok(Target {
        session,
        view,
        repository,
        draft,
    })
}

/// Runs the command of `arguments`, prints what it wrote and how it ended, and writes its changes
/// into the view.
pub(super) async fn run(
    context: &Context,
    arguments: &RunArguments,
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Result<(), Failure> {
    let mut target = target(context, arguments)
        .await
        .map_err(|failure| not_started(&failure))?;
    let api = target.session.api(context);
    let prefix = arguments.prefix.clone().or_else(|| {
        target
            .view
            .as_ref()
            .filter(|view| view.draft == target.draft && !view.prefix.is_empty())
            .map(|view| view.prefix.clone())
    });
    let request = StartRun {
        command: arguments.command.join(" "),
        prefix,
        add: arguments.add.clone(),
        outputs: arguments.outputs.clone(),
    };
    let started = api
        .start_run(&target.repository, &target.draft, &request)
        .await
        .map_err(|failure| not_started(&failure))?;
    let run = wait(&api, &target.repository, started.id).await?;
    report(context, &mut target, run, json, out, err).await
}

/// Waits for the run `id` to end. An interrupt cancels it, and a second gives up waiting.
async fn wait(api: &Api<'_>, repository: &RepositoryRef, id: u64) -> Result<Run, Failure> {
    let interrupt = tokio::signal::ctrl_c();
    let mut interrupt = std::pin::pin!(interrupt);
    let mut cancelled = false;
    loop {
        let polled = tokio::select! {
            polled = api.run(repository, id, WAIT) => polled,
            _ = &mut interrupt => {
                if cancelled {
                    return Err(lost(format!("interrupted again; run {id} may still be ending")));
                }
                cancelled = true;
                // A run that has ended cannot be cancelled; the next read says how it ended.
                let _ = api.cancel_run(repository, id).await;
                interrupt.set(tokio::signal::ctrl_c());
                continue;
            }
        };
        let run = polled.map_err(|failure| lost(format!("lost run {id}: {failure}")))?;
        if run.state == RunState::Ended {
            return Ok(run);
        }
    }
}

/// The bytes of `stream`, written to `sink`.
async fn pass_on(
    context: &Context,
    stream: Option<&RunStreamRead>,
    name: &str,
    sink: &mut impl Write,
) -> Result<(), String> {
    let Some(stream) = stream else {
        return Ok(());
    };
    let bytes = download(context, &stream.download)
        .await
        .map_err(|failure| format!("cannot read the run's {name}: {failure}"))?;
    sink.write_all(&bytes)
        .and_then(|()| sink.flush())
        .map_err(|error| format!("cannot write the run's {name}: {error}"))?;
    Ok(())
}

/// Prints the ended `run`, writes its changes into the view, and says by the exit code how it went.
async fn report(
    context: &Context,
    target: &mut Target,
    run: Run,
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Result<(), Failure> {
    let id = run.id;
    let Some(result) = &run.result else {
        return Err(lost(format!("run {id} ended without a result")));
    };
    let mut unread = Vec::new();
    if json {
        print_json(out, &without_grants(&run))?;
    } else {
        if let Err(message) = pass_on(context, result.stdout.as_ref(), "output", out).await {
            unread.push(message);
        }
        if let Err(message) = pass_on(context, result.stderr.as_ref(), "error output", err).await {
            unread.push(message);
        }
    }
    match result.outcome {
        RunOutcome::Exited => {
            if let Some(code) = result.exit_code {
                writeln!(err, "exit {code}")?;
            }
        }
        RunOutcome::TimedOut => writeln!(err, "run timed out")?,
        RunOutcome::Cancelled => writeln!(err, "run cancelled")?,
        RunOutcome::ExecutorFailed => writeln!(err, "run failed in its executor")?,
    }
    if result.outcome == RunOutcome::Exited
        && let Some(reason) = result.write_back.reason
    {
        writeln!(err, "changes not written back: {}", reason.text())?;
        for path in &result.write_back.paths {
            writeln!(err, "  {path}")?;
        }
    }
    let applied = apply(context, target, result, err).await;
    for message in &unread {
        writeln!(err, "ch: {message}")?;
    }
    let code = match (result.outcome, result.write_back.reason) {
        (RunOutcome::Exited, None) => match result.exit_code {
            Some(code) if unread.is_empty() && applied.is_ok() => {
                u8::try_from(code).unwrap_or(EXIT_RUN_LOST)
            }
            Some(_) | None => EXIT_RUN_LOST,
        },
        (RunOutcome::Exited, Some(NotWrittenReason::WriteDenied)) => EXIT_RUN_WRITE_DENIED,
        (
            RunOutcome::Exited,
            Some(NotWrittenReason::WorkspaceMoved | NotWrittenReason::InstanceClosed),
        ) => EXIT_RUN_CONFLICT,
        (RunOutcome::TimedOut | RunOutcome::Cancelled, _) => EXIT_RUN_STOPPED,
        (
            RunOutcome::Exited,
            Some(
                NotWrittenReason::TimedOut
                | NotWrittenReason::Cancelled
                | NotWrittenReason::ExecutorFailed,
            ),
        )
        | (RunOutcome::ExecutorFailed, _) => EXIT_RUN_LOST,
    };
    let message = match applied {
        Err(failure) if code == EXIT_RUN_LOST => Some(failure.to_string()),
        Err(_) | Ok(()) => None,
    };
    if code == 0 {
        return Ok(());
    }
    Err(Failure::Run { code, message })
}

/// `run` as the server answered it, without the download grants.
fn without_grants(run: &Run) -> serde_json::Value {
    let mut value = serde_json::to_value(run).unwrap_or(serde_json::Value::Null);
    for stream in ["stdout", "stderr"] {
        if let Some(object) = value
            .pointer_mut(&format!("/result/{stream}"))
            .and_then(serde_json::Value::as_object_mut)
        {
            object.remove("download");
        }
    }
    value
}

/// Writes the changes `result` wrote back into the view, where the view is of the run's
/// workspace and its file is still what it last recorded; names each file it leaves.
async fn apply(
    context: &Context,
    target: &mut Target,
    result: &RunResult,
    err: &mut impl Write,
) -> Result<(), Failure> {
    let Target {
        session,
        view: Some(view),
        repository,
        draft,
    } = target
    else {
        return Ok(());
    };
    if view.draft != *draft || result.write_back.outcome != WriteBackOutcome::Written {
        return Ok(());
    }
    let changes: Vec<&RunChange> = result
        .changes
        .iter()
        .filter(|change| {
            result.write_back.paths.contains(&change.path) && view.covers(Path::new(&change.path))
        })
        .collect();
    if changes.is_empty() {
        return Ok(());
    }
    let api = session.api(context);
    let workspace = api.workspace(repository, draft).await?;
    let mut outcome = Ok(());
    for change in changes {
        let held = view.held(&change.path);
        let step = match held {
            Ok(held) if held.as_ref() == view.recorded.get(&change.path) => {
                write_change(context, &api, view, &workspace.changes, change).await
            }
            Ok(_) => {
                writeln!(
                    err,
                    "{} was edited in the view during the run; it is left as it is",
                    change.path
                )?;
                Ok(())
            }
            Err(failure) => Err(failure),
        };
        if let Err(failure) = step {
            outcome = Err(failure);
            break;
        }
    }
    view.save()?;
    outcome
}

/// Writes `change` into the view and records it.
async fn write_change(
    context: &Context,
    api: &Api<'_>,
    view: &mut View,
    changed: &[crate::wire::ChangedPath],
    change: &RunChange,
) -> Result<(), Failure> {
    let path = change.path.as_str();
    let kind = match change.kind {
        RunChangeKind::Deleted => {
            let target = crate::view::file_path(&view.root, path)?;
            match std::fs::remove_file(&target) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(Failure::Other(format!("{}: {error}", target.display())));
                }
            }
            view.recorded.remove(path);
            return Ok(());
        }
        RunChangeKind::Regular => Kind::Regular,
        RunChangeKind::Executable => Kind::Executable,
    };
    let bytes = changed_bytes(context, api, view, changed, path).await?;
    let content = address(&bytes);
    if change.address.as_deref() != Some(content.as_str()) {
        return Err(Failure::Other(format!(
            "the workspace's {path} is not what the run wrote"
        )));
    }
    write_file(&view.root, path, kind, &bytes)?;
    view.recorded
        .insert(path.to_owned(), Entry { kind, content });
    Ok(())
}

/// The bytes the workspace holds at `path`: its change's, or else its base's where the run put
/// the base's content back.
async fn changed_bytes(
    context: &Context,
    api: &Api<'_>,
    view: &View,
    changed: &[crate::wire::ChangedPath],
    path: &str,
) -> Result<Vec<u8>, Failure> {
    if let Some(grant) = changed
        .iter()
        .find(|held| held.path == path)
        .and_then(|held| held.download.as_ref())
    {
        return download(context, grant).await;
    }
    let base = view
        .base
        .as_deref()
        .ok_or_else(|| Failure::Other(format!("the workspace holds no {path}")))?;
    let files = api.files(&view.repository, base, path).await?.files;
    let file = files
        .iter()
        .find(|file| file.path == path)
        .ok_or_else(|| Failure::Other(format!("the workspace holds no {path}")))?;
    download(context, &file.download).await
}

/// Downloads the file, or the files under the directory, `arguments.path` of a run's output tree.
pub(super) async fn fetch(context: &Context, arguments: &Fetch) -> Result<(), Failure> {
    let view = View::find(&std::env::current_dir()?)?;
    let session = Session::open(context).await?;
    let (repository, draft) = match &view {
        Some(view) => (view.repository.clone(), view.draft.clone()),
        None => (session.repository()?.clone(), DEFAULT_DRAFT.to_owned()),
    };
    let api = session.api(context);
    let draft = arguments.draft.clone().unwrap_or(draft);
    let id = match arguments.run {
        Some(id) => id,
        None => api.latest_run(&repository, &draft).await?.id,
    };
    let wanted = arguments.path.trim_matches('/');
    let destination = match (&arguments.destination, wanted.rsplit('/').next()) {
        (Some(destination), _) => destination.clone(),
        (None, Some(name)) if !name.is_empty() => PathBuf::from(name),
        (None, _) => {
            return Err(Failure::Usage(
                "fetch of the whole output tree needs a destination".to_owned(),
            ));
        }
    };
    let files = api.run_files(&repository, id, wanted).await?.files;
    if files.is_empty() {
        return Err(Failure::Denied(format!(
            "run {id} has no {wanted} in its output tree"
        )));
    }
    let mut targets = Vec::new();
    for file in &files {
        let relative = file
            .path
            .strip_prefix(wanted)
            .map(|rest| rest.trim_start_matches('/'))
            .ok_or_else(|| Failure::Other(format!("{} is not under {wanted}", file.path)))?;
        let target = if relative.is_empty() {
            destination.clone()
        } else {
            contained(&destination, relative)?
        };
        targets.push((file, target));
    }
    for (file, target) in targets {
        let bytes = download(context, &file.download).await?;
        if address(&bytes) != file.address {
            return Err(Failure::Other(format!(
                "{} is not what the run's output tree holds",
                file.path
            )));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| Failure::Other(format!("{}: {error}", parent.display())))?;
        }
        std::fs::write(&target, &bytes)
            .map_err(|error| Failure::Other(format!("{}: {error}", target.display())))?;
        let mode = if file.executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode))
            .map_err(|error| Failure::Other(format!("{}: {error}", target.display())))?;
    }
    Ok(())
}

/// `relative` below `destination`.
///
/// # Errors
///
/// Fails where `relative` would reach outside `destination`.
fn contained(destination: &Path, relative: &str) -> Result<PathBuf, Failure> {
    let path = Path::new(relative);
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        Ok(destination.join(path))
    } else {
        Err(Failure::Other(format!(
            "{relative:?} would be written outside {}",
            destination.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_path_below_the_destination_and_refuses_one_that_climbs_out() {
        let destination = Path::new("out");
        assert_eq!(contained(destination, "a/b").unwrap(), Path::new("out/a/b"));
        for relative in ["../a", "a/../../b", "/etc/passwd"] {
            assert!(contained(destination, relative).is_err(), "{relative}");
        }
    }
}
