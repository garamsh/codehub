//! A PostgreSQL database created for one test.

use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::{AssertSqlSafe, Connection, Executor, PgConnection};

use crate::{env, unique};

/// A database created for one test and dropped with it.
pub struct TestDatabase {
    server: String,
    name: String,
}

impl TestDatabase {
    /// Creates an empty database named `codehub_test_<unique>` on the server
    /// `CODEHUB_TEST_POSTGRES_URL` names.
    ///
    /// # Panics
    ///
    /// Panics if the variable is not set, the server cannot be reached or the database cannot be
    /// created.
    pub async fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("{}_{nanos}", unique("codehub_test").replace('-', "_"));
        let server = env("CODEHUB_TEST_POSTGRES_URL");
        let mut admin = PgConnection::connect(&server).await.unwrap();
        admin
            .execute(AssertSqlSafe(format!("create database {name}")))
            .await
            .unwrap();
        Self { server, name }
    }

    /// The URL of the database: the server's with the database replaced.
    ///
    /// # Panics
    ///
    /// Panics if the server's URL names no database.
    #[must_use]
    pub fn url(&self) -> String {
        let (base, _) = self
            .server
            .rsplit_once('/')
            .expect("the URL names a database");
        format!("{base}/{}", self.name)
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let (server, name) = (self.server.clone(), self.name.clone());
        // A thread of its own, since the test's runtime may be shutting down.
        let removed = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let mut admin = PgConnection::connect(&server).await.unwrap();
                    admin
                        .execute(AssertSqlSafe(format!(
                            "drop database if exists {name} with (force)"
                        )))
                        .await
                        .unwrap();
                });
        });
        let _ = removed.join();
    }
}
