//! Retrying a statement that failed for a reason that passes (ADR 0054), with a bounded backoff.
//!
//! Two classes are kept apart. An idempotent statement, a read or a write that leaves the same
//! rows however often it runs, is retried on every transient error. Any other statement is retried
//! only where the error proves nothing was applied: the pool had no connection to send it on, the
//! server rolled it back for a serialization failure or a deadlock (SQLSTATE 40001, 40P01), or it
//! refused it on a standby (25006). A commit of unknown outcome is in neither class and is not
//! retried blindly.

use std::time::Duration;

use tokio::sync::Notify;

/// Woken where a statement failed in a way a failover explains, so the supervisor of each
/// [`Database`](crate::Database) looks for the primary at once and does not wait for its next poll.
/// It is shared by the databases of a process: a supervisor woken for another's failure asks its
/// own server, finds it healthy and waits again.
pub(crate) static FAILOVER_SUSPECTED: Notify = Notify::const_new();

/// The attempts made in all, the first included.
const ATTEMPTS: u32 = 4;

/// The wait before the second attempt; it doubles up to [`LONGEST`].
const FIRST: Duration = Duration::from_millis(50);

/// Whether `error` proves the statement that failed with it was not applied.
pub(crate) fn nothing_applied(error: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(error) = error {
        return error
            .code()
            .is_some_and(|code| matches!(&*code, "40001" | "40P01" | "25006"));
    }
    matches!(error, sqlx::Error::PoolTimedOut)
}

/// Whether `error` is one a failover explains: a standby refused the statement (25006), or the
/// connection was lost or the server went away.
fn failover_suspected(error: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(error) = error {
        return error.code().is_some_and(|code| {
            matches!(&*code, "25006" | "57P01" | "57P02" | "57P03") || code.starts_with("08")
        });
    }
    matches!(
        error,
        sqlx::Error::Io(_) | sqlx::Error::Tls(_) | sqlx::Error::WorkerCrashed
    )
}

/// Wakes the supervisors where `error` is one a failover explains.
pub(crate) fn suspect_failover(error: &sqlx::Error) {
    if failover_suspected(error) {
        FAILOVER_SUSPECTED.notify_waiters();
    }
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
            Ok(value) => return Ok(value),
            Err(error) => {
                suspect_failover(&error);
                if attempt >= ATTEMPTS || !retryable(&error) {
                    return Err(error);
                }
                tokio::time::sleep(wait).await;
                wait *= 2;
                attempt += 1;
            }
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
    async fn a_failure_a_failover_explains_wakes_the_supervisors_before_the_statement_is_retried() {
        let woken = FAILOVER_SUSPECTED.notified();
        tokio::pin!(woken);
        woken.as_mut().enable();

        let _: Result<(), _> = idempotent(|| async { Err(sqlx::Error::WorkerCrashed) }).await;

        assert!(
            tokio::time::timeout(Duration::from_millis(10), woken)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_failure_no_failover_explains_wakes_no_one() {
        let woken = FAILOVER_SUSPECTED.notified();
        tokio::pin!(woken);
        woken.as_mut().enable();

        let _: Result<(), _> = idempotent(|| async { Err(sqlx::Error::PoolTimedOut) }).await;

        assert!(
            tokio::time::timeout(Duration::from_millis(10), woken)
                .await
                .is_err()
        );
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
