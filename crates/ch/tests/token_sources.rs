//! `ch` with a token from a file or a variable, and the RFC 8693 exchange, against a server that
//! verifies tokens with the real OIDC adapter and the test Keycloak.

#[cfg(test)]
pub mod common;

use common::{Run, User, env, file, serve_oidc, unique};
use test_support::keycloak_token;

/// What `ch` prints on a successful `org create`.
#[cfg(test)]
const CREATED: &str = "created organisation";

/// The token endpoint of the test realm.
#[cfg(test)]
fn endpoint() -> String {
    format!(
        "{}/protocol/openid-connect/token",
        env("CODEHUB_TEST_KEYCLOAK_ISSUER")
    )
}

/// Runs `ch` as `user` with `variables` set, and waits for it.
#[cfg(test)]
async fn run(user: &User, variables: &[(&str, &str)], arguments: &[&str]) -> Run {
    let mut command = user.command(arguments);
    command.envs(variables.iter().copied());
    let output = command.output().await.unwrap();
    Run {
        code: output.status.code(),
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// The arguments of an exchange through the test realm's `codehub-exchange` client, with `secret`.
#[cfg(test)]
fn exchange_arguments(secret: &str) -> Vec<String> {
    [
        "--exchange-endpoint",
        &endpoint(),
        "--exchange-audience",
        "codehub",
        "--exchange-client-id",
        &env("CODEHUB_TEST_KEYCLOAK_EXCHANGE_CLIENT_ID"),
        "--exchange-client-secret",
        secret,
    ]
    .map(str::to_owned)
    .to_vec()
}

/// `ch` arguments: the global ones, then the verb.
#[cfg(test)]
fn with<'a>(globals: &'a [String], verb: &[&'a str]) -> Vec<&'a str> {
    globals
        .iter()
        .map(String::as_str)
        .chain(verb.iter().copied())
        .collect()
}

#[tokio::test]
async fn presents_a_token_from_a_file() {
    let server = serve_oidc().await;
    let user = User::logged_out();
    let token = keycloak_token("TEST", "alice").await;
    let path = file(format!("\n  {token} \n").as_bytes());

    let out = user
        .ok(&[
            "--server",
            server.url(),
            "--token-file",
            &path,
            "org",
            "create",
            &unique("org"),
        ])
        .await;

    assert!(out.starts_with(CREATED), "{out}");
    assert!(!user.state_file().exists());
}

#[tokio::test]
async fn presents_a_token_from_a_variable() {
    let server = serve_oidc().await;
    let user = User::logged_out();
    let token = keycloak_token("TEST", "alice").await;

    let run = run(
        &user,
        &[("CH_TEST_TOKEN", &token)],
        &[
            "--server",
            server.url(),
            "--token-env",
            "CH_TEST_TOKEN",
            "org",
            "create",
            &unique("org"),
        ],
    )
    .await;

    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(run.out().starts_with(CREATED));
}

#[tokio::test]
async fn takes_the_source_and_the_server_from_the_environment() {
    let server = serve_oidc().await;
    let user = User::logged_out();
    let token = keycloak_token("TEST", "alice").await;
    let path = file(token.as_bytes());

    let run = run(
        &user,
        &[("CH_TOKEN_FILE", &path), ("CH_SERVER", server.url())],
        &["org", "create", &unique("org")],
    )
    .await;

    assert_eq!(run.code, Some(0), "{}", run.stderr);
}

#[tokio::test]
async fn exchanges_a_token_the_server_refuses_on_its_own() {
    let server = serve_oidc().await;
    let user = User::logged_out();
    let upstream = keycloak_token("UPSTREAM", "alice").await;
    let path = file(upstream.as_bytes());

    let direct = user
        .ch(&[
            "--server",
            server.url(),
            "--token-file",
            &path,
            "org",
            "create",
            &unique("org"),
        ])
        .await;
    assert_eq!(direct.code, Some(3), "{}", direct.stderr);

    let globals = exchange_arguments(&env("CODEHUB_TEST_KEYCLOAK_EXCHANGE_CLIENT_SECRET"));
    let mut arguments = vec!["--server", server.url(), "--token-file", &path];
    arguments.extend(globals.iter().map(String::as_str));
    let name = unique("org");
    arguments.extend(["org", "create", &name]);
    let out = user.ok(&arguments).await;
    assert!(out.starts_with(CREATED), "{out}");
}

#[tokio::test]
async fn keeps_the_exchanged_token_out_of_the_state_file() {
    let server = serve_oidc().await;
    let user = User::logged_out();
    let upstream = keycloak_token("UPSTREAM", "alice").await;
    let path = file(upstream.as_bytes());
    let globals = exchange_arguments(&env("CODEHUB_TEST_KEYCLOAK_EXCHANGE_CLIENT_SECRET"));
    let base = [
        "--server".to_owned(),
        server.url().to_owned(),
        "--token-file".to_owned(),
        path,
    ];
    let globals: Vec<String> = base.into_iter().chain(globals).collect();
    let org = unique("org");

    user.ok(&with(&globals, &["use", &format!("{org}/app")]))
        .await;
    user.ok(&with(&globals, &["org", "create", &org])).await;

    let state = std::fs::read_to_string(user.state_file()).unwrap();
    assert!(state.contains(&format!("{org}/app")), "{state}");
    assert!(!state.contains("token"), "{state}");
    assert!(!state.contains(&upstream), "{state}");
    assert!(!state.contains("eyJ"), "{state}");
}

#[tokio::test]
async fn fails_the_exchange_naming_the_reason() {
    let server = serve_oidc().await;
    let user = User::logged_out();
    let upstream = keycloak_token("UPSTREAM", "alice").await;
    let path = file(upstream.as_bytes());
    let globals = exchange_arguments("not-the-secret");
    let mut arguments = vec!["--server", server.url(), "--token-file", &path];
    arguments.extend(globals.iter().map(String::as_str));
    let name = unique("org");
    arguments.extend(["org", "create", &name]);

    let run = user.ch(&arguments).await;

    assert_eq!(run.code, Some(3), "{}", run.stderr);
    assert!(
        run.stderr.contains("token exchange failed"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("unauthorized_client"), "{}", run.stderr);
    assert!(!run.stderr.contains("not-the-secret"), "{}", run.stderr);
}

#[tokio::test]
async fn exits_3_naming_a_source_that_gives_no_token() {
    let server = serve_oidc().await;
    let user = User::logged_out();
    let empty = file(b" \n");
    let missing = format!("{empty}.missing");

    for (arguments, named) in [
        (vec!["--token-file", missing.as_str()], missing.clone()),
        (vec!["--token-file", empty.as_str()], empty.clone()),
        (
            vec!["--token-env", "CH_TEST_NEVER_SET"],
            "CH_TEST_NEVER_SET".to_owned(),
        ),
    ] {
        let mut all = vec!["--server", server.url()];
        all.extend(arguments);
        all.extend(["org", "create", "x"]);
        let run = user.ch(&all).await;
        assert_eq!(run.code, Some(3), "{all:?}: {}", run.stderr);
        assert!(run.stderr.contains(&named), "{all:?}: {}", run.stderr);
    }
}

#[tokio::test]
async fn refuses_both_sources_and_an_exchange_without_one() {
    let user = User::logged_out();
    let both = user
        .ch(&[
            "--token-file",
            "a",
            "--token-env",
            "B",
            "--server",
            "http://127.0.0.1:1",
            "org",
            "create",
            "x",
        ])
        .await;
    assert_eq!(both.code, Some(2), "{}", both.stderr);

    let alone = user
        .ch(&[
            "--exchange-endpoint",
            "http://127.0.0.1:1/token",
            "--exchange-audience",
            "codehub",
            "org",
            "create",
            "x",
        ])
        .await;
    assert_eq!(alone.code, Some(2), "{}", alone.stderr);
    assert!(alone.stderr.contains("token source"), "{}", alone.stderr);
}

#[tokio::test]
async fn documents_every_option_and_hides_the_secret() {
    let user = User::logged_out();
    let secret = "hunter2-secret";

    let run = run(&user, &[("CH_EXCHANGE_CLIENT_SECRET", secret)], &["--help"]).await;

    assert_eq!(run.code, Some(0));
    let help = run.out();
    for name in [
        "--token-file",
        "CH_TOKEN_FILE",
        "--token-env",
        "CH_TOKEN_ENV",
        "--server",
        "CH_SERVER",
        "--exchange-endpoint",
        "CH_EXCHANGE_ENDPOINT",
        "--exchange-audience",
        "CH_EXCHANGE_AUDIENCE",
        "--exchange-client-id",
        "CH_EXCHANGE_CLIENT_ID",
        "--exchange-client-secret",
        "CH_EXCHANGE_CLIENT_SECRET",
    ] {
        assert!(help.contains(name), "{name} is not in --help");
    }
    assert!(!help.contains(secret));
}
