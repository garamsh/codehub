//! The deployment contract names every option of the runner.

use clap::CommandFactory;

#[test]
fn deployment_md_names_every_runner_option_and_no_other() {
    test_support::assert_contract(&codehub_runner::Config::command(), "Runner options");
}
