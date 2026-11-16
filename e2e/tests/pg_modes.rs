//! The PostgreSQL operating modes of ADR 0054 against the profile `pg-modes` of `compose.yaml`: a
//! primary with a synchronous standby, and PgBouncer in transaction mode in front of the primary.
//! `make test-pg-modes` starts the profile and runs these; `e2e` skips them.
//!
//! The failover test kills the primary, so it runs after the others.

#[cfg(test)]
pub mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::{Server, TestDatabase, User, env, unique};

/// How long a step that waits for the stack to change may take.
const BOUND: Duration = Duration::from_secs(90);

/// Runs `docker compose` against the profile with `arguments` and returns what it printed.
#[cfg(test)]
async fn compose(arguments: &[&str]) -> String {
    let output = tokio::process::Command::new("docker")
        .args(["compose", "--env-file"])
        .arg(env("CODEHUB_PG_MODES_ENV"))
        .arg("-f")
        .arg(env("CODEHUB_PG_MODES_COMPOSE"))
        .args([
            "-p",
            &env("CODEHUB_PG_MODES_PROJECT"),
            "--profile",
            "pg-modes",
        ])
        .args(arguments)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "docker compose {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Starts `codehub` against `url`, with `more` set as well.
fn start(database: &TestDatabase, url: String, more: &[(&'static str, String)]) -> Server {
    let mut settings = vec![
        ("CODEHUB_OIDC_AUDIENCE", "ch".to_owned()),
        ("CODEHUB_POSTGRES_URL", url),
    ];
    settings.extend(more.iter().cloned());
    Server::start_with(database, settings)
}

/// The status `/readyz` of `server` answers with, 0 where it does not answer.
#[cfg(test)]
async fn readyz(server: &Server) -> u16 {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    match client.get(format!("{}/readyz", server.base)).send().await {
        Ok(response) => response.status().as_u16(),
        Err(_) => 0,
    }
}

/// Waits until `ready` holds, polling twice a second.
///
/// # Panics
///
/// Panics, with `what`, if it does not hold within the bound.
async fn until<F: AsyncFn() -> bool>(what: &str, ready: F) {
    let deadline = Instant::now() + BOUND;
    while !ready().await {
        assert!(Instant::now() < deadline, "{what} did not happen in time");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The content landed at `f<n>.txt`.
fn content(n: usize) -> String {
    format!("landing {n}\n")
}

/// Stages and lands `f<n>.txt` in a draft of its own; whether the server acknowledged it.
async fn land(user: &User, n: usize) -> bool {
    let (draft, path) = (format!("d{n}"), format!("f{n}.txt"));
    if user.put(&draft, &path, &content(n)).await.code != Some(0) {
        return false;
    }
    user.ch(&["land", "-m", &path, "--draft", &draft])
        .await
        .code
        == Some(0)
}

#[tokio::test]
async fn pg_modes_pooler_serves_landings_and_reads_through_a_transaction_pooler() {
    let database = TestDatabase::new().await;
    // A pooler cannot hold a session lock for the migration, so it runs on a direct connection.
    // The pooler tracks the prepared statements of its clients, which the default statement cache
    // of 100 needs.
    let server = start(
        &database,
        env("CODEHUB_TEST_PG_MODES_POOLER_URL"),
        &[(
            "CODEHUB_POSTGRES_MIGRATE_URL",
            env("CODEHUB_TEST_PG_MODES_FAILOVER_URL"),
        )],
    );
    let alice = User::new("alice");
    alice.login(&server).await;
    alice.create_repository(&unique("org"), "app").await;

    // Concurrent landings on disjoint paths each swap `main`, which the pooler may send to any
    // backend; every one of them lands.
    let (a, b, c, d) = tokio::join!(
        land(&alice, 0),
        land(&alice, 1),
        land(&alice, 2),
        land(&alice, 3)
    );
    assert!(a && b && c && d);
    for n in 0..4 {
        assert_eq!(alice.ok(&["read", &format!("f{n}.txt")]).await, content(n));
    }
    assert_eq!(readyz(&server).await, 200);
}

#[tokio::test]
async fn pg_modes_failover_keeps_every_acknowledged_landing_and_recovers_without_a_restart() {
    let database = TestDatabase::new().await;
    let server = start(
        &database,
        env("CODEHUB_TEST_PG_MODES_FAILOVER_URL"),
        &[("CODEHUB_POSTGRES_ACQUIRE_TIMEOUT", "3".to_owned())],
    );
    let alice = User::new("alice");
    alice.login(&server).await;
    alice.create_repository(&unique("org"), "app").await;
    assert_eq!(readyz(&server).await, 200);

    let stop = AtomicBool::new(false);
    let acknowledged = AtomicUsize::new(0);
    let load = async {
        let mut landed = Vec::new();
        let mut n = 0;
        while !stop.load(Ordering::Relaxed) {
            if land(&alice, n).await {
                landed.push(n);
                acknowledged.fetch_add(1, Ordering::Relaxed);
            }
            n += 1;
        }
        landed
    };
    let failover = async {
        until("three landings before the failure", async || {
            acknowledged.load(Ordering::Relaxed) >= 3
        })
        .await;

        // The primary dies, and the standby has not been promoted: no host is writable.
        compose(&["kill", "pg-primary"]).await;
        until("readiness reporting no writable primary", async || {
            readyz(&server).await == 503
        })
        .await;

        // The standby is promoted; the server finds it and serves again, with no restart. The
        // promoted server has no standby of its own, so its commits stop waiting for one, as a
        // failover tool would arrange.
        let user = env("CODEHUB_TEST_POSTGRES_USER");
        compose(&[
            "exec",
            "-T",
            "pg-standby",
            "psql",
            "-U",
            &user,
            "-d",
            "postgres",
            "-c",
            "select pg_promote()",
            "-c",
            "alter system set synchronous_standby_names = ''",
            "-c",
            "select pg_reload_conf()",
        ])
        .await;
        until("readiness after the promotion", async || {
            readyz(&server).await == 200
        })
        .await;
        let before = acknowledged.load(Ordering::Relaxed);
        until("landings after the promotion", async || {
            acknowledged.load(Ordering::Relaxed) >= before + 3
        })
        .await;
        stop.store(true, Ordering::Relaxed);
    };
    let (landed, ()) = tokio::join!(load, failover);

    // Every landing the server acknowledged is on `main` of the promoted standby.
    assert!(landed.len() >= 6);
    for n in landed {
        assert_eq!(
            alice.ok(&["read", &format!("f{n}.txt")]).await,
            content(n),
            "landing {n} was acknowledged and is not on main"
        );
    }
}
