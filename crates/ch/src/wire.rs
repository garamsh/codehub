//! The request and response bodies of the HTTP API (ADR 0024), declared here and checked against
//! `crates/adapter-http/openapi.json` by `tests/wire.rs`. Each type takes the name of its schema.

use serde::{Deserialize, Serialize};

/// The name of an organisation or repository to create.
#[derive(Debug, Serialize, Deserialize)]
pub struct NewName {
    /// The name.
    pub name: String,
}

/// An organisation or repository the server created.
#[derive(Debug, Serialize, Deserialize)]
pub struct Created {
    /// The name.
    pub name: String,
}

/// An issuer and a subject, which together name a principal.
#[derive(Debug, Serialize, Deserialize)]
pub struct Principal {
    /// The issuer URL.
    pub issuer: String,
    /// The subject at that issuer.
    pub subject: String,
}

/// A member's role in an organisation.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// May manage members.
    Admin,
    /// May use what its grants cover.
    Member,
}

/// What a grant allows.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// Read the prefix.
    Read,
    /// Write, and so read, the prefix.
    Write,
}

/// A principal and its role in an organisation.
#[derive(Debug, Serialize, Deserialize)]
pub struct Member {
    /// The issuer URL.
    pub issuer: String,
    /// The subject at that issuer.
    pub subject: String,
    /// The role.
    pub role: Role,
}

/// Access of a principal to the files under a prefix of a repository.
#[derive(Debug, Serialize, Deserialize)]
pub struct Grant {
    /// The issuer URL.
    pub issuer: String,
    /// The subject at that issuer.
    pub subject: String,
    /// The directory covered; empty for the whole repository.
    pub prefix: String,
    /// What the grant allows.
    pub access: Access,
}

/// A request for a grant to upload an object.
#[derive(Debug, Serialize, Deserialize)]
pub struct Upload {
    /// The object's address, `sha256:` and 64 hex digits.
    pub address: String,
    /// The object's size in bytes.
    pub size: u64,
}

/// The method of a transfer grant.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TransferMethod {
    /// Download.
    Get,
    /// Upload.
    Put,
}

/// One header a transfer must carry.
#[derive(Debug, Serialize, Deserialize)]
pub struct Header {
    /// The header name.
    pub name: String,
    /// The header value.
    pub value: String,
}

/// Permission to move object bytes to or from the object store.
#[derive(Debug, Serialize, Deserialize)]
pub struct TransferGrant {
    /// Where to send the request.
    pub url: String,
    /// The method to send it with.
    pub method: TransferMethod,
    /// The headers to send with it, and no others.
    pub headers: Vec<Header>,
    /// Seconds since 1970-01-01T00:00:00Z.
    pub expires_at: i64,
}

/// The kind of a file.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// An ordinary file.
    Regular,
    /// A file with the executable bit.
    Executable,
}

/// A file staged in a draft.
#[derive(Debug, Serialize, Deserialize)]
pub struct StagedFile {
    /// The kind of the file.
    pub kind: Kind,
    /// The address of its bytes.
    pub content: String,
}

/// The message a draft lands with.
#[derive(Debug, Serialize, Deserialize)]
pub struct Landing {
    /// The message.
    pub message: String,
}

/// The changeset a landing made.
#[derive(Debug, Serialize, Deserialize)]
pub struct Landed {
    /// The changeset's address.
    pub changeset: String,
}

/// A file at the head of the repository.
#[derive(Debug, Serialize, Deserialize)]
pub struct File {
    /// The kind of the file.
    pub kind: Kind,
    /// Where its bytes can be downloaded.
    pub download: TransferGrant,
}

/// One version of a file.
#[derive(Debug, Serialize, Deserialize)]
pub struct Version {
    /// The changeset that made it.
    pub changeset: String,
    /// The file's path.
    pub path: String,
    /// The kind of the file.
    pub kind: Kind,
    /// The address of its bytes.
    pub content: String,
    /// The changesets this version follows.
    pub previous: Vec<String>,
}

/// The versions of a file, newest first.
#[derive(Debug, Serialize, Deserialize)]
pub struct History {
    /// The versions.
    pub versions: Vec<Version>,
}

/// An RFC 9457 problem document.
#[derive(Debug, Serialize, Deserialize)]
pub struct Problem {
    /// Always `about:blank`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The status text.
    pub title: String,
    /// The HTTP status code.
    pub status: u16,
    /// What went wrong.
    pub detail: String,
    /// The paths a 403 or a 409 is about.
    #[serde(default)]
    pub paths: Vec<String>,
}
