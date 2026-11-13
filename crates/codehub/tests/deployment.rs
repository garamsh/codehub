//! The deployment contract and the release tooling that ships with it.

use clap::CommandFactory;
use std::process::Command;

#[test]
fn deployment_md_names_every_server_setting_and_no_other() {
    test_support::assert_contract(&codehub::Config::command(), "Server settings");
}

/// The version `scripts/release-version.sh` prints for `tag`, or its refusal.
fn release_version(tag: &str) -> Result<String, String> {
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/release-version.sh"
    );
    let out = Command::new("sh").arg(script).arg(tag).output().unwrap();
    if out.status.success() {
        Ok(String::from_utf8(out.stdout).unwrap().trim_end().to_owned())
    } else {
        Err(String::from_utf8(out.stderr).unwrap())
    }
}

#[test]
fn the_release_version_is_the_tag_without_its_v() {
    assert_eq!(release_version("v1.2.3"), Ok("1.2.3".to_owned()));
    assert_eq!(release_version("v0.10.20"), Ok("0.10.20".to_owned()));
}

#[test]
fn the_release_version_refuses_a_tag_that_is_not_vx_y_z() {
    for tag in [
        "v1.2",
        "1.2.3",
        "v01.2.3",
        "v1.2.3-rc1",
        "v1.2.3.4",
        "v1.2.3\nx",
        "latest",
        "",
    ] {
        assert!(release_version(tag).is_err(), "{tag:?} was accepted");
    }
}
