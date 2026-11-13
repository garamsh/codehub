//! The `ch` command-line client and its composition site.

pub mod api;
pub mod cli;
pub mod command;
pub mod failure;
pub mod login;
pub mod state;
pub mod token;
pub mod view;
pub mod wire;

use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;
use std::time::Duration;

use crate::cli::Cli;
use crate::command::{Context, execute};
use crate::failure::EXIT_OTHER;
use crate::state::StateFile;

/// Runs `ch` over the arguments after the program name, reporting on stdout and stderr.
///
/// This is the composition site: it builds the HTTP client and the state file the verbs run
/// against.
pub fn run<I: IntoIterator<Item = OsString>>(args: I) -> ExitCode {
    let cli = match Cli::try_parse_checked(std::iter::once(OsString::from("ch")).chain(args)) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            // Nowhere left to report a failed write.
            let _ = error.print();
            return ExitCode::from(u8::try_from(code).unwrap_or(EXIT_OTHER));
        }
    };
    let mut stderr = std::io::stderr();
    match run_verb(cli, &mut stderr) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            let _ = writeln!(stderr, "ch: {failure}");
            ExitCode::from(failure.exit_code())
        }
    }
}

fn run_verb(cli: Cli, stderr: &mut std::io::Stderr) -> Result<(), failure::Failure> {
    let path = state::path(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
    .ok_or_else(|| failure::Failure::Other("neither XDG_CONFIG_HOME nor HOME is set".to_owned()))?;
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()?;
    let context = Context {
        http,
        state: StateFile::at(path),
        credentials: cli.credentials.clone(),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut stdout = std::io::stdout().lock();
    let result = runtime.block_on(execute(cli, &context, &mut stdout, stderr));
    stdout.flush()?;
    result
}
