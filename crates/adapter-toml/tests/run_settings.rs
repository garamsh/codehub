//! The settings of a workspace's runs, read from `.codehub/run.toml` (ADR 0051).

use adapter_toml::TomlCheckDefinitions;
use app::domain::check::Timeout;
use app::domain::path::Path;
use app::port::driven::check_definitions::{CheckDefinitions, CheckFileError};

#[cfg(test)]
fn definitions() -> TomlCheckDefinitions {
    TomlCheckDefinitions::new().unwrap()
}

#[cfg(test)]
fn invalid(text: &str) -> String {
    match definitions().parse_run(text.as_bytes()) {
        Err(CheckFileError::Invalid { reason, .. }) => reason,
        other => panic!("expected an invalid file, got {other:?}"),
    }
}

#[test]
fn the_run_file_is_at_the_root_of_the_codehub_folder() {
    let expected: Path = ".codehub/run.toml".parse().unwrap();
    assert_eq!(definitions().run_file(), &expected);
}

#[test]
fn an_empty_file_means_the_defaults() {
    let settings = definitions().parse_run(b"").unwrap();
    assert!(settings.caches.is_empty());
    assert!(settings.labels.is_empty());
    assert_eq!(settings.timeout, Timeout::default_timeout());
    assert_eq!(settings.timeout.seconds(), 600);
}

#[test]
fn reads_caches_labels_and_the_timeout() {
    let text = "caches = [\"target\", \"node_modules\"]\nlabels = [\"linux\"]\ntimeout = 90\n";

    let settings = definitions().parse_run(text.as_bytes()).unwrap();

    let caches: Vec<String> = settings.caches.iter().map(ToString::to_string).collect();
    assert_eq!(caches, ["node_modules", "target"]);
    let labels: Vec<String> = settings.labels.iter().map(ToString::to_string).collect();
    assert_eq!(labels, ["linux"]);
    assert_eq!(settings.timeout.seconds(), 90);
}

#[test]
fn refuses_what_it_does_not_know_or_cannot_read() {
    assert!(invalid("command = \"make\"").contains("unknown key command"));
    assert!(invalid("timeout = \"slow\"").contains("timeout"));
    assert!(invalid("timeout = 0").contains("timeout"));
    assert!(invalid("caches = [\"a//b\"]").contains("caches"));
    assert!(matches!(
        definitions().parse_run(b"timeout = "),
        Err(CheckFileError::Syntax { line: 1, .. })
    ));
    assert!(matches!(
        definitions().parse_run(&[0xff, 0xfe]),
        Err(CheckFileError::Syntax { .. })
    ));
}
