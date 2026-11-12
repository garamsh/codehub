//! Source control: upload grants, staging, landing, reading and history.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use app::domain::address::Address;
use app::domain::changeset::Change;
use app::domain::file::{File, FileKind};
use app::domain::principal::Caller;
use app::domain::repository::RepositoryId;
use app::port::driven::object_store::{Grant, GrantMethod};
use app::port::driving::source_control::Landing;
use app::port::driving::source_control::SourceControl;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::Services;
use crate::extract::{Authenticated, Body, Params, Queried, find, parse};
use crate::problem::Problem;

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KindBody {
    Regular,
    Executable,
}

impl From<FileKind> for KindBody {
    fn from(kind: FileKind) -> Self {
        match kind {
            FileKind::Regular => Self::Regular,
            FileKind::Executable => Self::Executable,
        }
    }
}

impl From<KindBody> for FileKind {
    fn from(kind: KindBody) -> Self {
        match kind {
            KindBody::Regular => Self::Regular,
            KindBody::Executable => Self::Executable,
        }
    }
}

#[derive(Serialize)]
pub struct HeaderBody {
    name: String,
    value: String,
}

#[derive(Serialize)]
pub struct GrantBody {
    url: String,
    method: &'static str,
    headers: Vec<HeaderBody>,
    expires_at: i64,
}

impl From<Grant> for GrantBody {
    fn from(grant: Grant) -> Self {
        Self {
            url: grant.url,
            method: match grant.method {
                GrantMethod::Get => "GET",
                GrantMethod::Put => "PUT",
            },
            headers: grant
                .headers
                .into_iter()
                .map(|(name, value)| HeaderBody { name, value })
                .collect(),
            expires_at: grant.expires_at,
        }
    }
}

#[derive(Deserialize)]
pub struct UploadBody {
    address: String,
    size: u64,
}

#[derive(Deserialize)]
pub struct StageBody {
    kind: KindBody,
    content: String,
}

#[derive(Deserialize)]
pub struct LandBody {
    message: String,
}

#[derive(Serialize)]
pub struct LandedBody {
    changeset: String,
}

#[derive(Serialize)]
pub struct FileBody {
    kind: KindBody,
    download: GrantBody,
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    at: Option<String>,
}

#[derive(Serialize)]
pub struct VersionBody {
    changeset: String,
    path: String,
    kind: KindBody,
    content: String,
    previous: Vec<String>,
}

#[derive(Serialize)]
pub struct HistoryBody {
    versions: Vec<VersionBody>,
}

/// The server's current time, in seconds since 1970-01-01T00:00:00Z.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

pub async fn request_upload<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository)): Params<(String, String)>,
    Body(upload): Body<UploadBody>,
) -> Result<Json<GrantBody>, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    let address = parse("address", &upload.address)?;
    let grant = ports
        .source_control()
        .request_upload(&caller, repository, address, upload.size)
        .await?;
    Ok(Json(grant.into()))
}

pub async fn stage_file<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, draft, path)): Params<(String, String, String, String)>,
    Body(file): Body<StageBody>,
) -> Result<StatusCode, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    let change = Change::File(File {
        kind: file.kind.into(),
        content: parse("address", &file.content)?,
    });
    stage(&*ports, &caller, repository, &draft, &path, change).await
}

pub async fn stage_deletion<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, draft, path)): Params<(String, String, String, String)>,
) -> Result<StatusCode, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    stage(
        &*ports,
        &caller,
        repository,
        &draft,
        &path,
        Change::Deletion,
    )
    .await
}

async fn stage<P: Services>(
    ports: &P,
    caller: &Caller,
    repository: RepositoryId,
    draft: &str,
    path: &str,
    change: Change,
) -> Result<StatusCode, Problem> {
    ports
        .source_control()
        .stage(
            caller,
            repository,
            &parse("draft name", draft)?,
            parse("path", path)?,
            change,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn land<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, draft)): Params<(String, String, String)>,
    Body(landing): Body<LandBody>,
) -> Result<Json<LandedBody>, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    let landing = Landing {
        message: landing.message,
        timestamp: now(),
    };
    let changeset = ports
        .source_control()
        .land(&caller, repository, &parse("draft name", &draft)?, landing)
        .await?;
    Ok(Json(LandedBody {
        changeset: changeset.to_string(),
    }))
}

pub async fn read_file<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, path)): Params<(String, String, String)>,
) -> Result<Json<FileBody>, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    let content = ports
        .source_control()
        .read_file(&caller, repository, &parse("path", &path)?)
        .await?;
    Ok(Json(FileBody {
        kind: content.kind.into(),
        download: content.download.into(),
    }))
}

pub async fn history<P: Services>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, path)): Params<(String, String, String)>,
    Queried(query): Queried<HistoryQuery>,
) -> Result<Json<HistoryBody>, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    let at = query
        .at
        .as_deref()
        .map(|at| parse::<Address>("address", at))
        .transpose()?;
    let versions = ports
        .source_control()
        .history(&caller, repository, at, &parse("path", &path)?)
        .await?;
    Ok(Json(HistoryBody {
        versions: versions
            .into_iter()
            .map(|node| VersionBody {
                changeset: node.changeset.to_string(),
                path: node.path.to_string(),
                kind: node.file.kind.into(),
                content: node.file.content.to_string(),
                previous: node.previous.iter().map(ToString::to_string).collect(),
            })
            .collect(),
    }))
}
