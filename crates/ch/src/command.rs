//! What each verb does: reads the local state, calls the API and prints the result.

use std::fmt::Write as _;
use std::io::Write;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::Client;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::api::{Api, transfer};
use crate::cli::{
    Cli, Command, Land, Login, MemberCommand, OrgCommand, Put, RepoCommand, Role as CliRole,
};
use crate::failure::Failure;
use crate::login;
use crate::state::{RepositoryRef, State, StateFile};
use crate::wire::{Access, Grant, Kind, Member, Principal, Role, StagedFile, Upload};

/// What the verbs run against: the HTTP client and the local state.
pub struct Context {
    /// The client for the server, the issuer and the object store.
    pub http: Client,
    /// The state file.
    pub state: StateFile,
}

/// Runs the verb of `cli`. `err` is where a prompt goes, `out` where the result does.
///
/// # Errors
///
/// Fails with what the verb could not do; [`Failure::exit_code`] says how.
pub async fn execute(
    cli: Cli,
    context: &Context,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Result<(), Failure> {
    let json = cli.json;
    match cli.command {
        Command::Login(arguments) => log_in(context, &arguments, json, out, err).await,
        Command::Use { repository } => select(context, repository),
        Command::Put(arguments) => put(context, &arguments).await,
        Command::Land(arguments) => land(context, &arguments, json, out).await,
        Command::Log { path } => log(context, &path, json, out).await,
        Command::Read { path } => read(context, &path, json, out).await,
        Command::Org {
            command: OrgCommand::Create { organisation },
        } => create_organisation(context, &organisation, json, out).await,
        Command::Repo {
            command: RepoCommand::Create { repository },
        } => create_repository(context, &repository, json, out).await,
        Command::Member { command } => member(context, command).await,
    }
}

/// The state and what it selects.
struct Session {
    state: State,
}

impl Session {
    fn open(context: &Context) -> Result<Self, Failure> {
        Ok(Self {
            state: context.state.require()?,
        })
    }

    fn api<'a>(&'a self, context: &'a Context) -> Api<'a> {
        Api::new(&context.http, &self.state.server, &self.state.token)
    }

    fn repository(&self) -> Result<&RepositoryRef, Failure> {
        self.state.repository.as_ref().ok_or_else(|| {
            Failure::Other("no repository selected; run `ch use <org>/<repo>`".to_owned())
        })
    }
}

fn print_json(out: &mut impl Write, value: &impl Serialize) -> Result<(), Failure> {
    let text = serde_json::to_string(value)
        .map_err(|error| Failure::Other(format!("cannot print JSON: {error}")))?;
    writeln!(out, "{text}")?;
    Ok(())
}

async fn log_in(
    context: &Context,
    arguments: &Login,
    json: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Result<(), Failure> {
    let flow = login::start(&context.http, &arguments.issuer, &arguments.client_id).await?;
    writeln!(err, "{}", flow.prompt())?;
    let token = flow.poll(&context.http).await?;
    let principal = login::principal(&token)?;
    let server = arguments.server.trim_end_matches('/').to_owned();
    let repository = context
        .state
        .load()
        .ok()
        .flatten()
        .filter(|state| state.server == server)
        .and_then(|state| state.repository);
    context.state.save(&State {
        server,
        token,
        repository,
    })?;
    if json {
        print_json(out, &principal)
    } else {
        writeln!(
            out,
            "logged in as {} at {}",
            principal.subject, principal.issuer
        )?;
        Ok(())
    }
}

fn select(context: &Context, repository: RepositoryRef) -> Result<(), Failure> {
    let mut state = context.state.require()?;
    state.repository = Some(repository);
    context.state.save(&state)
}

/// The address of `bytes`: `sha256:` and the digest in lowercase hex.
fn address(bytes: &[u8]) -> String {
    let mut address = String::from("sha256:");
    for byte in Sha256::digest(bytes) {
        // Writing to a String does not fail.
        let _ = write!(address, "{byte:02x}");
    }
    address
}

async fn put(context: &Context, arguments: &Put) -> Result<(), Failure> {
    let session = Session::open(context)?;
    let repository = session.repository()?;
    let api = session.api(context);
    match (arguments.delete, &arguments.file) {
        (false, Some(file)) => {
            let bytes = std::fs::read(file)
                .map_err(|error| Failure::Other(format!("{}: {error}", file.display())))?;
            let upload = Upload {
                address: address(&bytes),
                size: bytes.len() as u64,
            };
            let grant = api.request_upload(repository, &upload).await?;
            transfer(&context.http, &grant, bytes).await?;
            let staged = StagedFile {
                kind: if arguments.executable {
                    Kind::Executable
                } else {
                    Kind::Regular
                },
                content: upload.address,
            };
            api.stage(repository, &arguments.draft, &arguments.path, &staged)
                .await
        }
        (true, None) => {
            api.stage_deletion(repository, &arguments.draft, &arguments.path)
                .await
        }
        (true, Some(_)) | (false, None) => Err(Failure::Other(
            "put takes a file, or --delete and no file".to_owned(),
        )),
    }
}

async fn land(
    context: &Context,
    arguments: &Land,
    json: bool,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let session = Session::open(context)?;
    let landed = session
        .api(context)
        .land(session.repository()?, &arguments.draft, &arguments.message)
        .await?;
    if json {
        print_json(out, &landed)
    } else {
        writeln!(out, "{}", landed.changeset)?;
        Ok(())
    }
}

async fn log(
    context: &Context,
    path: &str,
    json: bool,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let session = Session::open(context)?;
    let history = session
        .api(context)
        .history(session.repository()?, path)
        .await?;
    if json {
        return print_json(out, &history.versions);
    }
    for version in &history.versions {
        writeln!(out, "{} {}", version.changeset, version.content)?;
    }
    Ok(())
}

/// `read --json`: the file's path and kind, and its bytes as text where they are UTF-8 and as
/// base64 where they are not.
#[derive(Serialize)]
struct ReadOutput<'a> {
    path: &'a str,
    kind: Kind,
    #[serde(flatten)]
    content: Content,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum Content {
    Content(String),
    ContentBase64(String),
}

async fn read(
    context: &Context,
    path: &str,
    json: bool,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let session = Session::open(context)?;
    let file = session
        .api(context)
        .file(session.repository()?, path)
        .await?;
    let bytes = transfer(&context.http, &file.download, Vec::new()).await?;
    if !json {
        out.write_all(&bytes)?;
        return Ok(());
    }
    let content = match String::from_utf8(bytes) {
        Ok(text) => Content::Content(text),
        Err(error) => Content::ContentBase64(STANDARD.encode(error.into_bytes())),
    };
    print_json(
        out,
        &ReadOutput {
            path,
            kind: file.kind,
            content,
        },
    )
}

async fn create_organisation(
    context: &Context,
    organisation: &str,
    json: bool,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let session = Session::open(context)?;
    let created = session
        .api(context)
        .create_organisation(organisation)
        .await?;
    if json {
        print_json(out, &created)
    } else {
        writeln!(out, "created organisation {}", created.name)?;
        Ok(())
    }
}

async fn create_repository(
    context: &Context,
    repository: &RepositoryRef,
    json: bool,
    out: &mut impl Write,
) -> Result<(), Failure> {
    let session = Session::open(context)?;
    let created = session.api(context).create_repository(repository).await?;
    if json {
        print_json(out, &created)
    } else {
        writeln!(
            out,
            "created repository {}/{}",
            repository.organisation, created.name
        )?;
        Ok(())
    }
}

/// The issuer `--issuer` names, or else the stored token's.
fn issuer(given: Option<String>, state: &State) -> Result<String, Failure> {
    match given {
        Some(issuer) => Ok(issuer),
        None => Ok(login::principal(&state.token)?.issuer),
    }
}

/// The grants the `--read` and `--write` prefixes name.
fn grants(principal: &Principal, read: Vec<String>, write: Vec<String>) -> Vec<Grant> {
    let named = |prefixes: Vec<String>, access: Access| {
        prefixes.into_iter().map(move |prefix| Grant {
            issuer: principal.issuer.clone(),
            subject: principal.subject.clone(),
            prefix,
            access,
        })
    };
    named(read, Access::Read)
        .chain(named(write, Access::Write))
        .collect()
}

async fn member(context: &Context, command: MemberCommand) -> Result<(), Failure> {
    let session = Session::open(context)?;
    let repository = session.repository()?;
    let api = session.api(context);
    match command {
        MemberCommand::Add {
            subject,
            issuer: given,
            role,
            read,
            write,
        } => {
            let principal = Principal {
                issuer: issuer(given, &session.state)?,
                subject,
            };
            let member = Member {
                issuer: principal.issuer.clone(),
                subject: principal.subject.clone(),
                role: match role {
                    CliRole::Admin => Role::Admin,
                    CliRole::Member => Role::Member,
                },
            };
            api.put_member(&repository.organisation, &member).await?;
            for grant in grants(&principal, read, write) {
                api.change_grant(repository, &grant, true).await?;
            }
            Ok(())
        }
        MemberCommand::Remove {
            subject,
            issuer: given,
            read,
            write,
        } => {
            let principal = Principal {
                issuer: issuer(given, &session.state)?,
                subject,
            };
            if read.is_empty() && write.is_empty() {
                return api
                    .delete_member(&repository.organisation, &principal)
                    .await;
            }
            for grant in grants(&principal, read, write) {
                api.change_grant(repository, &grant, false).await?;
            }
            Ok(())
        }
    }
}
