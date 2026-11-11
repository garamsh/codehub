//! `ch login` against the Dex of the check services, up to the prompt for the device code. The
//! user's approval needs a browser, so the grant is not completed here.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

#[cfg(test)]
fn config_home() -> std::path::PathBuf {
    let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("ch-login-{}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[cfg(test)]
fn login(issuer: &str, config_home: &std::path::Path) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_ch"))
        .args(["login", "http://127.0.0.1:1", "--issuer", issuer])
        .env("XDG_CONFIG_HOME", config_home)
        .env_remove("HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

#[test]
fn prompts_for_the_device_code_and_stores_nothing_until_it_is_approved() {
    let issuer = std::env::var("CODEHUB_TEST_OIDC_ISSUER")
        .unwrap_or_else(|_| panic!("CODEHUB_TEST_OIDC_ISSUER is not set; run through `make test`"));
    let home = config_home();
    let mut child = login(&issuer, &home);
    let stderr = child.stderr.take().unwrap();
    let prompt: Vec<String> = BufReader::new(stderr)
        .lines()
        .map(Result::unwrap)
        .take_while(|line| !line.is_empty())
        .take(2)
        .collect();
    child.kill().unwrap();
    child.wait().unwrap();

    let prompt = prompt.join("\n");
    assert!(prompt.contains(&issuer), "{prompt}");
    let code = prompt.lines().next().unwrap().rsplit(' ').next().unwrap();
    let (left, right) = code
        .split_once('-')
        .unwrap_or_else(|| panic!("no user code: {prompt}"));
    assert!(
        [left, right]
            .iter()
            .all(|part| part.len() == 4 && part.chars().all(char::is_alphanumeric)),
        "{prompt}"
    );
    assert!(!home.join("ch").join("state.json").exists());
}

#[test]
fn fails_with_the_other_code_where_the_issuer_cannot_be_reached() {
    let home = config_home();
    let output = login("http://127.0.0.1:1/nothing", &home)
        .wait_with_output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("ch: "), "{stderr}");
    assert!(!stderr.contains("To log in"), "{stderr}");
}
