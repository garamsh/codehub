//! The workspace: opening, reading, describing, snapshotting, restoring and diffing a draft, and
//! listing the files of a changeset (ADR 0048).

use std::collections::BTreeMap;
use std::sync::Arc;

use app::domain::address::Address;
use app::domain::changeset::Change;
use app::domain::diff::ContentDiff;
use app::domain::draft::{ChangeStatus, Edit};
use app::domain::file::File;
use app::domain::name::DraftName;
use app::domain::path::Path;
use app::port::driving::source_control::{ListedFile, PathDiff, SourceControl, Workspace};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::Services;
use crate::extract::{Authenticated, Body, Params, Queried, find, parse, parse_prefix};
use crate::problem::Problem;
use crate::source_control::{GrantBody, KindBody};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusBody {
    Added,
    Modified,
    Deleted,
}

impl From<ChangeStatus> for StatusBody {
    fn from(status: ChangeStatus) -> Self {
        match status {
            ChangeStatus::Added => Self::Added,
            ChangeStatus::Modified => Self::Modified,
            ChangeStatus::Deleted => Self::Deleted,
        }
    }
}

#[derive(Serialize)]
pub struct ChangedPathBody {
    path: String,
    status: StatusBody,
    kind: KindBody,
    download: Option<GrantBody>,
}

#[derive(Serialize)]
pub struct WorkspaceBody {
    base: Option<String>,
    description: String,
    changes: Vec<ChangedPathBody>,
}

impl From<Workspace> for WorkspaceBody {
    fn from(workspace: Workspace) -> Self {
        Self {
            base: workspace.base.map(|base| base.to_string()),
            description: workspace.description,
            changes: workspace
                .changes
                .into_iter()
                .map(|changed| ChangedPathBody {
                    path: changed.path.to_string(),
                    status: changed.status.into(),
                    kind: changed.kind.into(),
                    download: changed.download.map(GrantBody::from),
                })
                .collect(),
        }
    }
}

#[derive(Deserialize)]
pub struct DescriptionBody {
    description: String,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
enum ActionBody {
    File { kind: KindBody, content: String },
    Delete,
    Restore,
}

#[derive(Deserialize)]
pub struct SnapshotChangeBody {
    path: String,
    #[serde(flatten)]
    action: ActionBody,
}

#[derive(Deserialize)]
pub struct SnapshotBody {
    changes: Vec<SnapshotChangeBody>,
}

#[derive(Deserialize)]
pub struct DiffQuery {
    path: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentDiffBody {
    Lines { unified: String },
    Binary,
    Kind,
}

#[derive(Serialize)]
pub struct PathDiffBody {
    path: String,
    status: StatusBody,
    diff: ContentDiffBody,
}

#[derive(Serialize)]
pub struct DiffBody {
    diffs: Vec<PathDiffBody>,
}

impl From<PathDiff> for PathDiffBody {
    fn from(diff: PathDiff) -> Self {
        Self {
            path: diff.path.to_string(),
            status: diff.status.into(),
            diff: match diff.diff {
                ContentDiff::Lines(unified) => ContentDiffBody::Lines { unified },
                ContentDiff::Binary => ContentDiffBody::Binary,
                ContentDiff::KindOnly => ContentDiffBody::Kind,
            },
        }
    }
}

#[derive(Deserialize)]
pub struct FilesQuery {
    prefix: Option<String>,
}

#[derive(Serialize)]
pub struct ListedFileBody {
    path: String,
    kind: KindBody,
    content: String,
    download: GrantBody,
}

#[derive(Serialize)]
pub struct FilesBody {
    files: Vec<ListedFileBody>,
}

impl From<ListedFile> for ListedFileBody {
    fn from(file: ListedFile) -> Self {
        Self {
            path: file.path.to_string(),
            kind: file.kind.into(),
            content: file.content.to_string(),
            download: file.download.into(),
        }
    }
}

fn draft_name(text: &str) -> Result<DraftName, Problem> {
    parse("draft name", text)
}

pub async fn open<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, draft)): Params<(String, String, String)>,
) -> Result<Json<WorkspaceBody>, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    let workspace = ports
        .source_control()
        .open_workspace(&caller, repository, &draft_name(&draft)?)
        .await?;
    Ok(Json(workspace.into()))
}

pub async fn read<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, draft)): Params<(String, String, String)>,
) -> Result<Json<WorkspaceBody>, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    let workspace = ports
        .source_control()
        .workspace(&caller, repository, &draft_name(&draft)?)
        .await?;
    Ok(Json(workspace.into()))
}

pub async fn describe<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, draft)): Params<(String, String, String)>,
    Body(description): Body<DescriptionBody>,
) -> Result<StatusCode, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    ports
        .source_control()
        .describe(
            &caller,
            repository,
            &draft_name(&draft)?,
            description.description,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn snapshot<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, draft)): Params<(String, String, String)>,
    Body(snapshot): Body<SnapshotBody>,
) -> Result<StatusCode, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    let draft = draft_name(&draft)?;
    let mut edits = BTreeMap::new();
    for change in snapshot.changes {
        let path: Path = parse("path", &change.path)?;
        let edit = match change.action {
            ActionBody::File { kind, content } => Edit::Record(Change::File(File {
                kind: kind.into(),
                content: parse("address", &content)?,
            })),
            ActionBody::Delete => Edit::Record(Change::Deletion),
            ActionBody::Restore => Edit::Restore,
        };
        if edits.insert(path, edit).is_some() {
            return Err(Problem::bad_request(format!(
                "the path {} is changed twice in one snapshot",
                change.path
            )));
        }
    }
    ports
        .source_control()
        .snapshot(&caller, repository, &draft, edits)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn restore<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, draft, path)): Params<(String, String, String, String)>,
) -> Result<StatusCode, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    ports
        .source_control()
        .restore(
            &caller,
            repository,
            &draft_name(&draft)?,
            &parse("path", &path)?,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn diff<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, draft)): Params<(String, String, String)>,
    Queried(query): Queried<DiffQuery>,
) -> Result<Json<DiffBody>, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    let path = query
        .path
        .as_deref()
        .map(|path| parse::<Path>("path", path))
        .transpose()?;
    let diffs = ports
        .source_control()
        .diff(&caller, repository, &draft_name(&draft)?, path.as_ref())
        .await?;
    Ok(Json(DiffBody {
        diffs: diffs.into_iter().map(PathDiffBody::from).collect(),
    }))
}

pub async fn list_files<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, changeset)): Params<(String, String, String)>,
    Queried(query): Queried<FilesQuery>,
) -> Result<Json<FilesBody>, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    let changeset: Address = parse("changeset", &changeset)?;
    let prefix = parse_prefix(query.prefix.as_deref().unwrap_or_default())?;
    let files = ports
        .source_control()
        .list_files(&caller, repository, changeset, &prefix)
        .await?;
    Ok(Json(FilesBody {
        files: files.into_iter().map(ListedFileBody::from).collect(),
    }))
}
