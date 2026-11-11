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
fn exits_non_zero_on_unknown_argument() -> io::Result<()> {
    let output = ch(&["--bogus"])?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.lines().count(), 1);
    assert!(stderr.contains("--bogus"));
    Ok(())
}

#[test]
fn exits_non_zero_without_arguments() -> io::Result<()> {
    let output = ch(&[])?;
    assert!(!output.status.success());
    assert!(!output.stderr.is_empty());
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
    assert!(!output.status.success());
    assert_ne!(output.status.code(), Some(101));
    assert_eq!(String::from_utf8_lossy(&output.stderr).lines().count(), 1);
    Ok(())
}
