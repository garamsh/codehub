//! The deployment contract.

use clap::CommandFactory;

#[test]
fn deployment_md_names_every_server_setting_and_no_other() {
    test_support::assert_contract(&codehub::Config::command(), "Server settings");
}
