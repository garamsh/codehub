//! The verbs on a workspace and its view (ADR 0048): `open`, and the verbs that snapshot the view
//! first.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::Serialize;

use super::{Context, Session, address, print_json};
use crate::api::{Api, transfer};
use crate::cli::Open;
use crate::failure::Failure;
use crate::view::{Entry, Phase, Rules, View, file_path, write_file};
use crate::wire::{
    ChangedPath, DiffContent, Kind, Snapshot, SnapshotChange, TransferGrant, Upload,
};

/// The bytes `grant` lets `ch` download.
async fn download(context: &Context, grant: &TransferGrant) -> Result<Vec<u8>, Failure> {
    transfer(&context.http, grant, Vec::new()).await
}

/// Opens the workspace and writes its view into `arguments.directory`.
pub(super) async fn open(context: &Context, arguments: &Open) -> Result<(), Failure> {
    let root = std::path::absolute(&arguments.directory)?;
    if let Some(outer) = View::find(&root)? {
        return Err(Failure::Usage(format!(
            "{} is inside the view at {}; open a view outside it",
            root.display(),
            outer.root.display()
        )));
    }
    match std::fs::read_dir(&root) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(Failure::Usage(format!(
                    "{} is not empty; open a view in an empty or absent directory",
                    root.display()
                )));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(Failure::Other(format!("{}: {error}", root.display()))),
    }
    let session = Session::open(context).await?;
    let repository = session.repository()?;
    let api = session.api(context);
    let prefix = arguments.prefix.trim_matches('/').to_owned();
    let workspace = api.open_workspace(repository, &arguments.draft).await?;
    let listed = match &workspace.base {
        Some(base) => api.files(repository, base, &prefix).await?.files,
        None => Vec::new(),
    };
    let mut view = View {
        root,
        repository: repository.clone(),
        draft: arguments.draft.clone(),
        base: workspace.base.clone(),
        prefix,
        phase: Phase::Open,
        base_files: BTreeMap::new(),
        recorded: BTreeMap::new(),
    };
    let changed: BTreeMap<&str, &ChangedPath> = workspace
        .changes
        .iter()
        .filter(|change| view.covers(Path::new(&change.path)))
        .map(|change| (change.path.as_str(), change))
        .collect();
    std::fs::create_dir_all(&view.root)?;
    for file in listed {
        let entry = Entry {
            kind: file.kind,
            content: file.content,
        };
        if !changed.contains_key(file.path.as_str()) {
            let bytes = download(context, &file.download).await?;
            write_file(&view.root, &file.path, entry.kind, &bytes)?;
            view.recorded.insert(file.path.clone(), entry.clone());
        }
        view.base_files.insert(file.path, entry);
    }
    for change in changed.into_values() {
        let Some(grant) = &change.download else {
            continue;
        };
        let bytes = download(context, grant).await?;
        write_file(&view.root, &change.path, change.kind, &bytes)?;
        view.recorded.insert(
            change.path.clone(),
            Entry {
                kind: change.kind,
                content: address(&bytes),
            },
        );
    }
    view.save()
}

/// Records what the view holds that its last snapshot did not.
async fn snapshot(context: &Context, api: &Api<'_>, view: &mut View) -> Result<(), Failure> {
    let rules = Rules::read(&view.root)?;
    let current = view.scan(&rules)?;
    let plan = view.plan(current);
    if plan.changes.is_empty() {
        return Ok(());
    }
    for change in &plan.changes {
        let SnapshotChange::File { path, content, .. } = change else {
            continue;
        };
        let target = file_path(&view.root, path)?;
        let bytes = std::fs::read(&target)
            .map_err(|error| Failure::Other(format!("{}: {error}", target.display())))?;
        if &address(&bytes) != content {
            return Err(Failure::Other(format!(
                "{path} changed while it was being snapshotted; run the command again"
            )));
        }
        let upload = Upload {
            address: content.clone(),
            size: bytes.len() as u64,
        };
        let grant = api.request_upload(&view.repository, &upload).await?;
        transfer(&context.http, &grant, bytes).await?;
    }
    let snapshot = Snapshot {
        changes: plan.changes,
    };
    api.snapshot(&view.repository, &view.draft, &snapshot)
        .await?;
    view.recorded = plan.recorded;
    view.save()
}

/// The session, and the open view of the working directory, snapshotted.
async fn begin(context: &Context) -> Result<(Session, View), Failure> {
    let mut view = View::find_open(&std::env::current_dir()?)?;
    let session = Session::open(context).await?;
    snapshot(context, &session.api(context), &mut view).await?;
    Ok((session, view))
}

/// `status --json`: the workspace as the server answers it, without the download grants.
#[derive(Serialize)]
struct StatusOutput<'a> {
    base: Option<&'a str>,
    description: &'a str,
    changes: Vec<ChangeOutput<'a>>,
}

#[derive(Serialize)]
struct ChangeOutput<'a> {
    path: &'a str,
    status: crate::wire::Status,
    kind: Kind,
}

pub(super) async fn status(
    context: &Context,
    json: bool,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let (session, view) = begin(context).await?;
    let workspace = session
        .api(context)
        .workspace(&view.repository, &view.draft)
        .await?;
    if json {
        return print_json(
            out,
            &StatusOutput {
                base: workspace.base.as_deref(),
                description: &workspace.description,
                changes: workspace
                    .changes
                    .iter()
                    .map(|change| ChangeOutput {
                        path: &change.path,
                        status: change.status,
                        kind: change.kind,
                    })
                    .collect(),
            },
        );
    }
    for change in &workspace.changes {
        writeln!(out, "{} {}", change.status.text(), change.path)?;
    }
    Ok(())
}

pub(super) async fn diff(
    context: &Context,
    path: Option<&str>,
    json: bool,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let (session, view) = begin(context).await?;
    let diff = session
        .api(context)
        .diff(&view.repository, &view.draft, path)
        .await?;
    if json {
        return print_json(out, &diff);
    }
    for changed in &diff.diffs {
        writeln!(out, "{} {}", changed.status.text(), changed.path)?;
        if let Some(change) = &changed.kind_change {
            writeln!(
                out,
                "kind {} -> {}",
                change.before.text(),
                change.after.text()
            )?;
        }
        match &changed.diff {
            DiffContent::Lines { unified } => {
                write!(out, "{unified}")?;
                if !unified.is_empty() && !unified.ends_with('\n') {
                    writeln!(out)?;
                }
            }
            DiffContent::Binary => writeln!(out, "binary files differ")?,
            DiffContent::Kind => {}
        }
    }
    Ok(())
}

pub(super) async fn describe(context: &Context, message: &str) -> Result<(), Failure> {
    let (session, view) = begin(context).await?;
    session
        .api(context)
        .describe(&view.repository, &view.draft, message)
        .await
}

/// Returns `path` to the base's content on the server and in the view.
pub(super) async fn restore(context: &Context, path: &str) -> Result<(), Failure> {
    let (session, mut view) = begin(context).await?;
    if !view.covers(Path::new(path)) {
        return Err(Failure::Usage(format!(
            "{path} is outside the view's prefix {:?}",
            view.prefix
        )));
    }
    let target = file_path(&view.root, path)?;
    let api = session.api(context);
    let held = view.base_files.get(path).cloned();
    let bytes = match (&held, &view.base) {
        (Some(_), Some(base)) => {
            let files = api.files(&view.repository, base, path).await?.files;
            let file = files
                .iter()
                .find(|file| file.path == path)
                .ok_or_else(|| Failure::Other(format!("the base no longer lists {path}")))?;
            Some(download(context, &file.download).await?)
        }
        (Some(_), None) | (None, _) => None,
    };
    api.restore(&view.repository, &view.draft, path).await?;
    // The state is saved before the view's file changes, so a failed write leaves the edit for
    // the next snapshot to send.
    match (held, bytes) {
        (Some(entry), Some(bytes)) => {
            view.recorded.insert(path.to_owned(), entry.clone());
            view.save()?;
            write_file(&view.root, path, entry.kind, &bytes)
        }
        (Some(_), None) | (None, _) => {
            view.recorded.remove(path);
            view.save()?;
            match std::fs::remove_file(&target) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(Failure::Other(format!("{}: {error}", target.display()))),
            }
        }
    }
}

/// Lands the workspace of `view`, with `message` or else its description, and closes the view.
pub(super) async fn land(
    context: &Context,
    view: View,
    message: Option<&str>,
    json: bool,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let mut view = view.ensure_open()?;
    let session = Session::open(context).await?;
    let api = session.api(context);
    snapshot(context, &api, &mut view).await?;
    let landed = api
        .land(&view.repository, &view.draft, message.unwrap_or_default())
        .await?;
    view.phase = Phase::Closed;
    view.save()?;
    if json {
        print_json(out, &landed)
    } else {
        writeln!(out, "{}", landed.changeset)?;
        Ok(())
    }
}
