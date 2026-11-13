//! `codehub-runner check-executor` end to end: the built binary runs one job through the host
//! executor with no server and no credential, and exits with the command's code.

use std::process::{Command, Output};

/// Runs the binary as `check-executor` with `args`, in an empty environment: no server, no token.
#[cfg(test)]
fn check(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_codehub-runner"))
        .arg("check-executor")
        .args(args)
        .env_clear()
        .output()
        .unwrap()
}

#[cfg(test)]
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn a_command_runs_with_no_server_and_its_output_follows_the_line_naming_the_end() {
    let output = check(&["--command", "echo x"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stdout(&output), "exited with code 0\nx\n");
}

#[test]
fn a_failing_command_exits_with_its_code() {
    let output = check(&["--command", "echo no; exit 4"]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(stdout(&output), "exited with code 4\nno\n");
}

#[test]
fn the_timeout_stops_a_long_command_and_the_stop_is_named() {
    let output = check(&["--command", "sleep 60", "--timeout", "1"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "stopped: timed out\n");
}

#[test]
fn an_executor_error_exits_125_with_the_error_on_standard_error() {
    let file = std::env::temp_dir().join(format!("check-executor-e2e-{}", std::process::id()));
    std::fs::write(&file, "a file, not a directory").unwrap();
    let under = file.join("under");
    let output = check(&["--command", "true", "--work-dir", under.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(125));
    assert_eq!(stdout(&output), "");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("the job's directory failed"), "{stderr}");
}

#[test]
fn the_gaon_executor_is_refused_at_start_without_its_settings() {
    let output = check(&["--command", "true", "--executor", "gaon"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--gaon-url"), "{stderr}");
}
