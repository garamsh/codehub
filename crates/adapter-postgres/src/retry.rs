//! Retrying a statement that failed for a reason that passes (ADR 0054), with a bounded backoff.
//!
//! Two classes are kept apart. An idempotent statement, a read or a write that leaves the same
//! rows however often it runs, is retried on every transient error. Any other statement is retried
//! only where the error proves nothing was applied: the pool had no connection to send it on, the
//! server rolled it back for a serialization failure or a deadlock (SQLSTATE 40001, 40P01), or it
//! refused it on a standby (25006). A commit of unknown outcome is in neither class and is not
//! retried blindly.

use std::time::Duration;

/// The attempts made in all, the first included.
const ATTEMPTS: u32 = 4;

/// The wait before the second attempt; it doubles up to [`LONGEST`].
const FIRST: Duration = Duration::from_millis(50);

/// The longest wait between two attempts.
const LONGEST: Duration = Duration::from_millis(400);

/// Whether `error` proves the statement that failed with it was not applied.
pub(crate) fn nothing_applied(error: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(error) = error {
        return error
            .code()
            .is_some_and(|code| matches!(&*code, "40001" | "40P01" | "25006"));
    }
    matches!(error, sqlx::Error::PoolTimedOut)
}

/// Whether `error` may pass on its own: the connection broke, the server restarted or went away
/// (SQLSTATE 57P01, 57P02, 57P03 and the class 08), or a standby refused the statement.
pub(crate) fn transient(error: &sqlx::Error) -> bool {
    if nothing_applied(error) {
        return true;
    }
    if let sqlx::Error::Database(error) = error {
        return error.code().is_some_and(|code| {
            matches!(&*code, "57P01" | "57P02" | "57P03") || code.starts_with("08")
        });
    }
    matches!(
        error,
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) | sqlx::Error::WorkerCrashed
    )
}

/// Runs `operation`, again on a transient error, for an operation that is idempotent.
pub(crate) async fn idempotent<T, Fut>(operation: impl FnMut() -> Fut) -> Result<T, sqlx::Error>
where
    Fut: Future<Output = Result<T, sqlx::Error>>,
{
    run(operation, transient).await
}

/// Runs `operation`, again only where the error proves nothing was applied.
pub(crate) async fn definite<T, Fut>(operation: impl FnMut() -> Fut) -> Result<T, sqlx::Error>
where
    Fut: Future<Output = Result<T, sqlx::Error>>,
{
    run(operation, nothing_applied).await
}

async fn run<T, Fut>(
    mut operation: impl FnMut() -> Fut,
    retryable: fn(&sqlx::Error) -> bool,
) -> Result<T, sqlx::Error>
where
    Fut: Future<Output = Result<T, sqlx::Error>>,
{
    let mut wait = FIRST;
    let mut attempt = 1;
    loop {
        match operation().await {
            Err(error) if attempt < ATTEMPTS && retryable(&error) => {
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(LONGEST);
                attempt += 1;
            }
            outcome => return outcome,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[tokio::test]
    async fn an_idempotent_statement_is_retried_on_a_broken_connection_up_to_the_bound() {
        let calls = Cell::new(0);
        let outcome: Result<(), _> = idempotent(|| async {
            calls.set(calls.get() + 1);
            Err(sqlx::Error::WorkerCrashed)
        })
        .await;
        assert!(outcome.is_err());
        assert_eq!(calls.get(), ATTEMPTS);
        let calls = Cell::new(0);
        let outcome = idempotent(|| async {
            calls.set(calls.get() + 1);
            if calls.get() < 3 {
                Err(sqlx::Error::PoolTimedOut)
            } else {
                Ok(7)
            }
        })
        .await;
        assert_eq!(outcome.unwrap(), 7);
    }

    #[tokio::test]
    async fn a_statement_that_may_have_been_applied_is_not_retried_blindly() {
        let calls = Cell::new(0);
        let outcome: Result<(), _> = definite(|| async {
            calls.set(calls.get() + 1);
            Err(sqlx::Error::WorkerCrashed)
        })
        .await;
        assert!(outcome.is_err());
        assert_eq!(calls.get(), 1);
        let calls = Cell::new(0);
        let _: Result<(), _> = definite(|| async {
            calls.set(calls.get() + 1);
            Err(sqlx::Error::PoolTimedOut)
        })
        .await;
        assert_eq!(calls.get(), ATTEMPTS);
    }

    #[tokio::test]
    async fn an_error_that_will_not_pass_is_returned_at_once() {
        let calls = Cell::new(0);
        let _: Result<(), _> = idempotent(|| async {
            calls.set(calls.get() + 1);
            Err(sqlx::Error::RowNotFound)
        })
        .await;
        assert_eq!(calls.get(), 1);
    }
}
