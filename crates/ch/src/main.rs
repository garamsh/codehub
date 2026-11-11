//! The `ch` binary: hands the process arguments to the composition site.

use std::process::ExitCode;

fn main() -> ExitCode {
    ch::run(std::env::args_os().skip(1))
}
