//! Problem details (RFC 9457) and the mapping from the ports' errors to them (ADR 0024).

use app::port::driven::authorisation_decision::AuthorisationError;
use app::port::driven::changeset_log::ChangesetLogError;
use app::port::driven::derived_data::DerivedDataError;
use app::port::driven::draft_store::DraftStoreError;
use app::port::driven::identity_verification::IdentityError;
use app::port::driven::job_queue::JobQueueError;
use app::port::driven::membership::{Access, MembershipError};
use app::port::driven::object_store::ObjectStoreError;
use app::port::driven::repository_directory::DirectoryError;
use app::port::driven::runner_registry::RunnerRegistryError;
use app::port::driving::access::AccessError;
use app::port::driving::authentication::AuthenticationError;
use app::port::driving::checks::{ChecksError, PlanError};
use app::port::driving::jobs::JobsError;
use app::port::driving::members::MembersError;
use app::port::driving::repositories::RepositoriesError;
use app::port::driving::runners::RunnersError;
use app::port::driving::runs::RunsError;
use app::port::driving::source_control::HistoryError;
use app::port::driving::source_control::LandError;
use app::port::driving::source_control::ListFilesError;
use app::port::driving::source_control::ReadFileError;
use app::port::driving::source_control::SourceControlError;
use app::port::driving::source_control::UploadError;
use app::port::driving::source_control::WorkspaceError;
use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// A failed request, answered as a problem document.
#[derive(Debug)]
pub struct Problem {
    status: StatusCode,
    detail: String,
    paths: Vec<String>,
    retry_after: Option<u64>,
}

impl Problem {
    fn new(status: StatusCode, detail: impl Into<String>) -> Self {
        Self {
            status,
            detail: detail.into(),
            paths: Vec::new(),
            retry_after: None,
        }
    }

    fn naming(
        status: StatusCode,
        detail: impl Into<String>,
        paths: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            status,
            detail: detail.into(),
            paths: paths.into_iter().collect(),
            retry_after: None,
        }
    }

    /// The request is malformed: a name, a path, an address or a body does not parse.
    pub(crate) fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, detail)
    }

    /// No valid bearer token came with the request.
    pub(crate) fn unauthorised(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, detail)
    }

    /// The thing the request names does not exist.
    pub(crate) fn not_found(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, detail)
    }

    /// No route answers the request.
    pub(crate) fn no_route() -> Self {
        Self::new(StatusCode::NOT_FOUND, "no such route")
    }

    /// A dependency of the server does not answer.
    pub(crate) fn unavailable(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, detail)
    }

    /// The caller sent more requests than its bucket allows; it may try again in `retry_after`
    /// seconds.
    pub(crate) fn rate_limited(retry_after: u64) -> Self {
        Self {
            retry_after: Some(retry_after),
            ..Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "too many requests; slow down",
            )
        }
    }

    /// The request body is larger than the server reads.
    pub(crate) fn payload_too_large() -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "the request body is larger than the server accepts",
        )
    }

    /// The process is serving as many requests as it takes at once; the caller may try again in
    /// `retry_after` seconds.
    pub(crate) fn overloaded(retry_after: u64) -> Self {
        Self {
            retry_after: Some(retry_after),
            ..Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "the server is serving as many requests as it can; try again",
            )
        }
    }

    /// The request took longer than the server allows.
    pub(crate) fn timed_out() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "the request took longer than the server allows",
        )
    }

    /// A failure of ours; the body names no internals.
    fn internal() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "the server failed")
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let mut body = json!({
            "type": "about:blank",
            "title": self.status.canonical_reason().unwrap_or("Error"),
            "status": self.status.as_u16(),
            "detail": self.detail,
        });
        if let (false, Value::Object(members)) = (self.paths.is_empty(), &mut body) {
            members.insert("paths".to_owned(), json!(self.paths));
        }
        let mut response = (self.status, Json(body)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        if let Some(seconds) = self.retry_after {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
        }
        if self.status == StatusCode::UNAUTHORIZED {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

impl From<IdentityError> for Problem {
    fn from(error: IdentityError) -> Self {
        match error {
            IdentityError::Rejected => Self::unauthorised("the token was rejected"),
            IdentityError::Backend(_) => Self::internal(),
        }
    }
}

impl From<AuthenticationError> for Problem {
    fn from(error: AuthenticationError) -> Self {
        match error {
            AuthenticationError::Identity(error) => error.into(),
        }
    }
}

impl From<AuthorisationError> for Problem {
    fn from(error: AuthorisationError) -> Self {
        match error {
            AuthorisationError::Backend(_) => Self::internal(),
        }
    }
}

impl From<DirectoryError> for Problem {
    fn from(error: DirectoryError) -> Self {
        match error {
            DirectoryError::OrganisationExists(name) => {
                Self::new(StatusCode::CONFLICT, format!("organisation {name} exists"))
            }
            DirectoryError::RepositoryExists(organisation, name) => Self::new(
                StatusCode::CONFLICT,
                format!("repository {organisation}/{name} exists"),
            ),
            DirectoryError::OrganisationNotFound(name) => {
                Self::new(StatusCode::NOT_FOUND, format!("no organisation {name}"))
            }
            DirectoryError::RepositoryNotFound(organisation, name) => Self::new(
                StatusCode::NOT_FOUND,
                format!("no repository {organisation}/{name}"),
            ),
            DirectoryError::Backend(_) => Self::internal(),
        }
    }
}

impl From<MembershipError> for Problem {
    fn from(error: MembershipError) -> Self {
        match error {
            MembershipError::NotAMember(principal) => Self::new(
                StatusCode::NOT_FOUND,
                format!("{principal} is not a member"),
            ),
            MembershipError::Backend(_) => Self::internal(),
        }
    }
}

impl From<AccessError> for Problem {
    fn from(error: AccessError) -> Self {
        match error {
            AccessError::RepositoryNotFound(_) => {
                Self::new(StatusCode::NOT_FOUND, "no such repository")
            }
            AccessError::Denied(_) => Self::new(StatusCode::FORBIDDEN, "the action is not allowed"),
            AccessError::PathsDenied { access, paths } => {
                let access = match access {
                    Access::Read => "read",
                    Access::Write => "write",
                    Access::Run => "run",
                };
                Self::naming(
                    StatusCode::FORBIDDEN,
                    format!("{access} is not allowed on these paths"),
                    paths.iter().map(ToString::to_string),
                )
            }
            AccessError::Directory(error) => error.into(),
            AccessError::Authorisation(error) => error.into(),
        }
    }
}

impl From<RepositoriesError> for Problem {
    fn from(error: RepositoriesError) -> Self {
        match error {
            RepositoriesError::Access(error) => error.into(),
            RepositoriesError::Directory(error) => error.into(),
            RepositoriesError::Membership(error) => error.into(),
        }
    }
}

impl From<MembersError> for Problem {
    fn from(error: MembersError) -> Self {
        match error {
            MembersError::Access(error) => error.into(),
            MembersError::Directory(error) => error.into(),
            MembersError::Membership(error) => error.into(),
            MembersError::RunGrantOnPrefix => Self::new(StatusCode::BAD_REQUEST, error.to_string()),
            MembersError::ExternallyAdministered => Self::new(
                StatusCode::FORBIDDEN,
                "members are administered outside codehub",
            ),
        }
    }
}

impl From<ObjectStoreError> for Problem {
    fn from(error: ObjectStoreError) -> Self {
        match error {
            ObjectStoreError::NotFound(address) => {
                Self::new(StatusCode::NOT_FOUND, format!("no object {address}"))
            }
            ObjectStoreError::Backend(_) => Self::internal(),
        }
    }
}

impl From<ChangesetLogError> for Problem {
    fn from(error: ChangesetLogError) -> Self {
        match error {
            ChangesetLogError::NotFound(address) => {
                Self::new(StatusCode::NOT_FOUND, format!("no changeset {address}"))
            }
            ChangesetLogError::RefMoved { .. } => {
                Self::new(StatusCode::CONFLICT, "main moved; try again")
            }
            ChangesetLogError::Encode(_) | ChangesetLogError::Backend(_) => Self::internal(),
        }
    }
}

impl From<DerivedDataError> for Problem {
    fn from(error: DerivedDataError) -> Self {
        match error {
            DerivedDataError::NotFound(address) => {
                Self::new(StatusCode::NOT_FOUND, format!("no changeset {address}"))
            }
            DerivedDataError::Backend(_) => Self::internal(),
        }
    }
}

impl From<DraftStoreError> for Problem {
    fn from(error: DraftStoreError) -> Self {
        match error {
            DraftStoreError::NotFound(draft) => {
                Self::new(StatusCode::NOT_FOUND, format!("no draft {}", draft.name))
            }
            DraftStoreError::Moved { .. } => {
                Self::new(StatusCode::CONFLICT, "the workspace changed; try again")
            }
            DraftStoreError::Backend(_) => Self::internal(),
        }
    }
}

impl From<UploadError> for Problem {
    fn from(error: UploadError) -> Self {
        match error {
            UploadError::TooLarge { size, max_size } => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("{size} bytes exceed the {max_size} byte maximum"),
            ),
            UploadError::Objects(error) => error.into(),
        }
    }
}

impl From<LandError> for Problem {
    fn from(error: LandError) -> Self {
        match error {
            LandError::Conflicts(conflicts) => Self::naming(
                StatusCode::CONFLICT,
                "the draft conflicts with changes landed on main since its base",
                conflicts.keys().map(ToString::to_string),
            ),
            LandError::MainMoved { .. } => {
                Self::new(StatusCode::CONFLICT, "main moved while landing; land again")
            }
            LandError::BaseNotOnMain => {
                Self::new(StatusCode::CONFLICT, "the draft's base is not on main")
            }
            LandError::Objects(error) => error.into(),
            LandError::Log(error) => error.into(),
            LandError::Derived(error) => error.into(),
            LandError::Drafts(error) => error.into(),
            LandError::Access(error) => error.into(),
            LandError::Tree(_)
            | LandError::Encode(_)
            | LandError::ChecksNotQueued { .. }
            | LandError::CacheRefs(_) => Self::internal(),
        }
    }
}

impl From<ReadFileError> for Problem {
    fn from(error: ReadFileError) -> Self {
        match error {
            ReadFileError::NotFound(path) => {
                Self::new(StatusCode::NOT_FOUND, format!("no file at {path}"))
            }
            ReadFileError::Objects(error) => error.into(),
            ReadFileError::Log(error) => error.into(),
            ReadFileError::Derived(error) => error.into(),
        }
    }
}

impl From<HistoryError> for Problem {
    fn from(error: HistoryError) -> Self {
        match error {
            HistoryError::Log(error) => error.into(),
            HistoryError::Derived(error) => error.into(),
            HistoryError::FileVersions(_) => Self::internal(),
        }
    }
}

impl From<WorkspaceError> for Problem {
    fn from(error: WorkspaceError) -> Self {
        match error {
            WorkspaceError::ObjectMissing(address) => Self::bad_request(format!(
                "the content {address} is not uploaded to this repository"
            )),
            WorkspaceError::Objects(error) => error.into(),
            WorkspaceError::Log(error) => error.into(),
            WorkspaceError::Derived(error) => error.into(),
            WorkspaceError::Drafts(error) => error.into(),
        }
    }
}

impl From<ListFilesError> for Problem {
    fn from(error: ListFilesError) -> Self {
        match error {
            ListFilesError::Objects(error) => error.into(),
            ListFilesError::Derived(error) => error.into(),
        }
    }
}

impl From<SourceControlError> for Problem {
    fn from(error: SourceControlError) -> Self {
        match error {
            SourceControlError::Access(error) => error.into(),
            SourceControlError::Upload(error) => error.into(),
            SourceControlError::Land(error) => error.into(),
            SourceControlError::ReadFile(error) => error.into(),
            SourceControlError::History(error) => error.into(),
            SourceControlError::Workspace(error) => error.into(),
            SourceControlError::ListFiles(error) => error.into(),
        }
    }
}

impl From<RunnerRegistryError> for Problem {
    fn from(error: RunnerRegistryError) -> Self {
        match error {
            RunnerRegistryError::NotFound(id) => Self::new(
                StatusCode::NOT_FOUND,
                format!("no runner registration {id}"),
            ),
            RunnerRegistryError::Backend(_) => Self::internal(),
        }
    }
}

impl From<JobQueueError> for Problem {
    fn from(error: JobQueueError) -> Self {
        match error {
            JobQueueError::NotFound(id) => Self::new(StatusCode::NOT_FOUND, format!("no job {id}")),
            JobQueueError::Finished(id) => {
                Self::new(StatusCode::CONFLICT, format!("job {id} has ended"))
            }
            JobQueueError::Backend(_) => Self::internal(),
        }
    }
}

impl From<RunnersError> for Problem {
    fn from(error: RunnersError) -> Self {
        match error {
            RunnersError::Access(error) => error.into(),
            RunnersError::Directory(error) => error.into(),
            RunnersError::Registry(error) => error.into(),
        }
    }
}

impl From<JobsError> for Problem {
    fn from(error: JobsError) -> Self {
        match error {
            JobsError::NotRegistered => Self::new(
                StatusCode::FORBIDDEN,
                "the caller is not a registered runner",
            ),
            JobsError::Queue(error) => error.into(),
            JobsError::Registry(error) => error.into(),
            JobsError::Access(error) => error.into(),
            JobsError::Drafts(error) => error.into(),
            JobsError::WrongKind(_) => Self::bad_request(
                "the report is for the other kind of job than the one the caller holds",
            ),
            JobsError::UnboundChanges(paths) => Self::bad_request(format!(
                "the report changes paths the run was not sent or allowed to add: {}",
                paths
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            JobsError::CacheRefs(_)
            | JobsError::Objects(_)
            | JobsError::Derived(_)
            | JobsError::Log(_)
            | JobsError::Path(_)
            | JobsError::Revert(_) => Self::internal(),
        }
    }
}

impl From<ChecksError> for Problem {
    fn from(error: ChecksError) -> Self {
        match error {
            ChecksError::Access(error) => error.into(),
            ChecksError::Queue(error) => error.into(),
            ChecksError::Log(error) => error.into(),
            ChecksError::Revert(_) => Self::internal(),
            ChecksError::Plan(error) => error.into(),
        }
    }
}

impl From<PlanError> for Problem {
    fn from(error: PlanError) -> Self {
        match error {
            PlanError::Rebase(error) => error.into(),
            PlanError::Drafts(error) => error.into(),
            PlanError::Derived(error) => error.into(),
            PlanError::Objects(error) => error.into(),
            PlanError::Queue(error) => error.into(),
            PlanError::Log(error) => error.into(),
            PlanError::Definitions { file, reason } => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("{file}: {reason}"),
            ),
        }
    }
}

impl From<RunsError> for Problem {
    fn from(error: RunsError) -> Self {
        match error {
            RunsError::Access(error) => error.into(),
            RunsError::NotFound(id) => Self::new(StatusCode::NOT_FOUND, format!("no run {id}")),
            RunsError::Finished(id) => {
                Self::new(StatusCode::CONFLICT, format!("run {id} has ended"))
            }
            RunsError::NoRun(draft) => Self::new(
                StatusCode::NOT_FOUND,
                format!("no run of {draft} has ended"),
            ),
            RunsError::AddOutsidePrefix(path) => {
                Self::bad_request(format!("{path} is not under the prefix"))
            }
            RunsError::RunFile { file, error } => Self::bad_request(format!("{file}: {error}")),
            RunsError::Drafts(error) => error.into(),
            RunsError::Derived(error) => error.into(),
            RunsError::Objects(error) => error.into(),
            RunsError::Log(error) => error.into(),
            RunsError::CacheRefs(_) => Self::internal(),
            RunsError::Queue(error) => error.into(),
        }
    }
}
