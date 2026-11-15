//! The deployment contract and the release tag check.

use clap::CommandFactory;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn deployment_md_names_every_server_setting_and_no_other() {
    test_support::assert_contract(&codehub::Config::command(), "Server settings");
}

#[cfg(test)]
/// The `run:` block of the step with `id: version` in `release.yml`, dedented.
fn version_step() -> String {
    let workflow = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.github/workflows/release.yml"
    ))
    .unwrap();
    let mut lines = workflow.lines().skip_while(|l| l.trim() != "- id: version");
    assert!(lines.next().is_some(), "no step with id: version");
    let mut lines = lines.skip_while(|l| l.trim() != "run: |");
    assert!(lines.next().is_some(), "the version step has no run block");
    lines
        .take_while(|l| l.trim().is_empty() || l.starts_with("          "))
        .map(|l| l.get(10..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
/// The version the `release.yml` version step writes for `tag`, or its stderr.
fn release_version(tag: &str) -> Result<String, String> {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    let output = std::env::temp_dir().join(format!(
        "release-version-{}-{}",
        std::process::id(),
        RUNS.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&output, "").unwrap();
    let out = Command::new("bash")
        .args(["-e", "-c", &version_step()])
        .env("REF_NAME", tag)
        .env("GITHUB_OUTPUT", &output)
        .output()
        .unwrap();
    let written = std::fs::read_to_string(&output).unwrap();
    std::fs::remove_file(&output).unwrap();
    if out.status.success() {
        let version = written.strip_prefix("version=").unwrap();
        Ok(version.trim_end().to_owned())
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
fn the_release_version_keeps_a_dev_or_rc_suffix() {
    assert_eq!(
        release_version("v0.1.0-dev.1"),
        Ok("0.1.0-dev.1".to_owned())
    );
    assert_eq!(
        release_version("v0.1.0-dev.12"),
        Ok("0.1.0-dev.12".to_owned())
    );
    assert_eq!(release_version("v0.1.0-rc.3"), Ok("0.1.0-rc.3".to_owned()));
}

#[test]
fn the_release_version_refuses_a_suffix_that_is_not_dev_or_rc_with_a_positive_number() {
    assert_eq!(
        release_version("v0.1.0-dev.1"),
        Ok("0.1.0-dev.1".to_owned()),
        "the control form is accepted"
    );
    for tag in [
        "v0.1.0-dev.0",
        "v0.1.0-dev.01",
        "v0.1.0-rc.007",
        "v0.1.0-beta.1",
        "v0.1.0-dev",
        "v0.1.0-rc",
        "v0.1.0-dev.",
        "v0.1.0-dev.1.2",
        "v0.1.0-dev.1a",
        "v0.1.0-DEV.1",
    ] {
        assert!(release_version(tag).is_err(), "{tag:?} was accepted");
    }
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
