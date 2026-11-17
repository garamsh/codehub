use app::port::driven::backlog::{Backlog, BacklogError, BacklogReading, QueueBacklog};
use sqlx::PgPool;

use crate::database::Database;
use crate::statements::Statements;

/// The backlog read from the queues of jobs and runs and from the directory outbox.
pub struct PostgresBacklog {
    pool: PgPool,
    statements: Statements,
}

impl PostgresBacklog {
    /// A backlog over `database`.
    #[must_use]
    pub fn new(database: &Database) -> Self {
        Self {
            pool: database.pool.clone(),
            statements: database.statements,
        }
    }
}

fn backend(error: impl ToString) -> BacklogError {
    BacklogError(error.to_string())
}

type Row = (String, i64, f64);

/// A job or run is queued from the last time its row changed: nothing but entering `queued` or
/// leaving it changes a queued row.
async fn queue(
    pool: &PgPool,
    statements: Statements,
    kind: &'static str,
    sql: &'static str,
) -> Result<Vec<QueueBacklog>, BacklogError> {
    let rows = statements
        .query_as::<Row>(sql)
        .fetch_all(pool)
        .await
        .map_err(backend)?;
    rows.into_iter()
        .map(|(labels, depth, age)| {
            Ok(QueueBacklog {
                kind,
                labels,
                depth: u64::try_from(depth).map_err(backend)?,
                oldest_age_seconds: age.max(0.0),
            })
        })
        .collect()
}

#[async_trait::async_trait]
impl Backlog for PostgresBacklog {
    async fn read(&self) -> Result<BacklogReading, BacklogError> {
        let mut queues = queue(
            &self.pool,
            self.statements,
            "job",
            "select array_to_string(check_labels, ','), count(*), \
                    coalesce(extract(epoch from now() - min(updated_at)), 0)::float8 \
             from jobs where state = 'queued' group by 1 order by 1",
        )
        .await?;
        queues.extend(
            queue(
                &self.pool,
                self.statements,
                "run",
                "select array_to_string(labels, ','), count(*), \
                        coalesce(extract(epoch from now() - min(updated_at)), 0)::float8 \
                 from runs where state = 'queued' group by 1 order by 1",
            )
            .await?,
        );
        let pending = self
            .statements
            .query_scalar::<i64>("select count(*) from directory_outbox")
            .fetch_one(&self.pool)
            .await
            .map_err(backend)?;
        Ok(BacklogReading {
            queues,
            outbox_pending: u64::try_from(pending).map_err(backend)?,
        })
    }
}
