//! Launches the built `ch` binary and checks what a caller observes.

use std::io;
use std::process::{Command, Output};

#[cfg(test)]
fn ch(args: &[&str]) -> io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_ch")).args(args).output()
}

#[test]
fn prints_name_and_version() -> io::Result<()> {
    let output = ch(&["--version"])?;
    assert!(output.status.success());
    let expected = format!("ch {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    Ok(())
}

#[test]
fn exits_with_the_usage_code_on_unknown_argument() -> io::Result<()> {
    let output = ch(&["--bogus"])?;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--bogus"));
    Ok(())
}

#[test]
fn exits_with_the_usage_code_without_arguments() -> io::Result<()> {
    let output = ch(&[])?;
    assert_eq!(output.status.code(), Some(2));
    assert!(!output.stderr.is_empty());
    Ok(())
}

#[test]
fn documents_the_exit_codes_in_help() -> io::Result<()> {
    let output = ch(&["--help"])?;
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    for line in [
        "3  Not logged in, or the server answered 401",
        "4  Landing rejected: 409 or 403 on land",
        "5  Denied or not found on any other operation: 403 or 404",
    ] {
        assert!(help.contains(line), "{line}");
    }
    Ok(())
}

#[cfg(test)]
fn help(args: &[&str]) -> io::Result<String> {
    let output = ch(args)?;
    assert!(output.status.success(), "{args:?}");
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[test]
fn describes_the_runner_and_checks_verbs_in_help() -> io::Result<()> {
    let top = help(&["--help"])?;
    assert!(top.contains("runner"), "{top}");
    assert!(top.contains("checks"), "{top}");
    assert!(
        top.contains("checks --wait ended with a blocking check"),
        "{top}"
    );

    let runner = help(&["runner", "--help"])?;
    for verb in ["register", "list", "remove"] {
        assert!(runner.contains(verb), "{verb}");
    }
    let register = help(&["runner", "register", "--help"])?;
    for flag in ["--issuer", "--label", "--repo"] {
        assert!(register.contains(flag), "{flag}");
    }
    assert!(register.contains("<SUBJECT>"));

    let checks = help(&["checks", "--help"])?;
    for flag in ["--wait", "--timeout", "[CHANGESET]", "head of `main`"] {
        assert!(checks.contains(flag), "{flag}");
    }
    assert!(!checks.contains("poll-interval"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn reports_non_unicode_argument_without_panicking() -> io::Result<()> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let output = Command::new(env!("CARGO_BIN_EXE_ch"))
        .arg(OsString::from_vec(vec![0xff]))
        .output()?;
    assert_eq!(output.status.code(), Some(2));
    assert!(!output.stderr.is_empty());
    Ok(())
}
