//! The command line: the verbs of ADR 0025 and their arguments.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::state::RepositoryRef;

const EXIT_CODES: &str = "\
Exit codes:
  0  Success
  1  Any other failure
  2  Usage
  3  Not logged in, or the server answered 401
  4  Landing rejected: 409 or 403 on land
  5  Denied or not found on any other operation: 403 or 404";

/// The `ch` command line.
#[derive(Debug, Parser)]
#[command(name = "ch", version, about, after_help = EXIT_CODES, arg_required_else_help = true)]
pub struct Cli {
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    pub json: bool,
    /// The verb.
    #[command(subcommand)]
    pub command: Command,
}

/// A verb.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Log in to a server through its identity issuer.
    Login(Login),
    /// Select the repository later verbs act on.
    Use {
        /// The repository, as `<org>/<repo>`.
        repository: RepositoryRef,
    },
    /// Stage a file, or its deletion, in a draft.
    Put(Put),
    /// Land a draft.
    Land(Land),
    /// List the versions of a file, newest first.
    Log {
        /// The file's path.
        path: String,
    },
    /// Write a file's bytes to stdout.
    Read {
        /// The file's path.
        path: String,
    },
    /// Act on organisations.
    Org {
        /// The verb.
        #[command(subcommand)]
        command: OrgCommand,
    },
    /// Act on repositories.
    Repo {
        /// The verb.
        #[command(subcommand)]
        command: RepoCommand,
    },
    /// Act on the members of the selected repository's organisation.
    Member {
        /// The verb.
        #[command(subcommand)]
        command: MemberCommand,
    },
}

/// Arguments of `login`.
#[derive(Debug, Args)]
pub struct Login {
    /// The server's base URL.
    pub server: String,
    /// The identity issuer's URL.
    #[arg(long)]
    pub issuer: String,
    /// The client id to log in as.
    #[arg(long, default_value = "ch")]
    pub client_id: String,
}

/// Arguments of `put`.
#[derive(Debug, Args)]
pub struct Put {
    /// The path in the repository.
    pub path: String,
    /// The local file whose bytes to stage.
    #[arg(required_unless_present = "delete", conflicts_with = "delete")]
    pub file: Option<PathBuf>,
    /// Stage the file with the executable bit.
    #[arg(long, conflicts_with = "delete")]
    pub executable: bool,
    /// Stage the deletion of the path.
    #[arg(long)]
    pub delete: bool,
    /// The draft to stage in.
    #[arg(long, default_value = "default")]
    pub draft: String,
}

/// Arguments of `land`.
#[derive(Debug, Args)]
pub struct Land {
    /// The landing message.
    #[arg(short, long)]
    pub message: String,
    /// The draft to land.
    #[arg(long, default_value = "default")]
    pub draft: String,
}

/// A verb on organisations.
#[derive(Debug, Subcommand)]
pub enum OrgCommand {
    /// Create an organisation.
    Create {
        /// The organisation's name.
        organisation: String,
    },
}

/// A verb on repositories.
#[derive(Debug, Subcommand)]
pub enum RepoCommand {
    /// Create a repository.
    Create {
        /// The repository, as `<org>/<repo>`.
        repository: RepositoryRef,
    },
}

/// A member's role in an organisation.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Role {
    /// May manage members.
    Admin,
    /// May use what its grants cover.
    Member,
}

/// A verb on members.
#[derive(Debug, Subcommand)]
pub enum MemberCommand {
    /// Make a principal a member of the organisation, and grant it access to the repository.
    Add {
        /// The member's subject.
        subject: String,
        /// The member's issuer; defaults to the issuer of the stored token.
        #[arg(long)]
        issuer: Option<String>,
        /// The role in the organisation.
        #[arg(long, value_enum, default_value = "member")]
        role: Role,
        /// A prefix to grant read access to; empty for the whole repository.
        #[arg(long)]
        read: Vec<String>,
        /// A prefix to grant write access to; empty for the whole repository.
        #[arg(long)]
        write: Vec<String>,
    },
    /// Remove a member, or only the grants named.
    Remove {
        /// The member's subject.
        subject: String,
        /// The member's issuer; defaults to the issuer of the stored token.
        #[arg(long)]
        issuer: Option<String>,
        /// A read grant to remove.
        #[arg(long)]
        read: Vec<String>,
        /// A write grant to remove.
        #[arg(long)]
        write: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("ch").chain(args.iter().copied()))
    }

    #[test]
    fn defaults_the_draft_and_takes_a_short_message() {
        let cli = parse(&["land", "-m", "text"]).unwrap();
        let Command::Land(land) = cli.command else {
            panic!("not land");
        };
        assert_eq!(
            (land.message.as_str(), land.draft.as_str()),
            ("text", "default")
        );
    }

    #[test]
    fn takes_json_before_or_after_the_verb() {
        assert!(parse(&["--json", "log", "a"]).unwrap().json);
        assert!(parse(&["log", "a", "--json"]).unwrap().json);
    }

    #[test]
    fn puts_a_file_or_a_deletion_and_not_both() {
        assert!(parse(&["put", "a", "f"]).is_ok());
        assert!(parse(&["put", "--delete", "a"]).is_ok());
        assert!(parse(&["put", "a"]).is_err());
        assert!(parse(&["put", "--delete", "a", "f"]).is_err());
        assert!(parse(&["put", "--delete", "--executable", "a"]).is_err());
    }

    #[test]
    fn takes_repeated_grants_and_the_empty_prefix() {
        let cli = parse(&[
            "member", "add", "bob", "--write", "", "--write", "src", "--read", "docs",
        ])
        .unwrap();
        let Command::Member {
            command: MemberCommand::Add { read, write, .. },
        } = cli.command
        else {
            panic!("not member add");
        };
        assert_eq!(write, ["", "src"]);
        assert_eq!(read, ["docs"]);
    }

    #[test]
    fn requires_the_issuer_to_log_in() {
        assert!(parse(&["login", "http://s"]).is_err());
        assert!(parse(&["login", "http://s", "--issuer", "http://i"]).is_ok());
    }

    #[test]
    fn rejects_a_repository_that_is_not_org_slash_repo() {
        assert!(parse(&["use", "acme"]).is_err());
        assert!(parse(&["use", "acme/app"]).is_ok());
    }
}
