use app::domain::repository::RepositoryId;
use app::port::driven::directory_changes::{ChangeKind, RepositoryChange};
use app::port::driven::directory_outbox::{DirectoryOutbox, DirectoryOutboxError};
use sqlx::PgPool;

use crate::conversion;
use crate::database::Database;

/// Serialises claims, so that two claimers never read one repository as claimable at once. It is
/// held for the claim only, never while the entries are delivered.
const CLAIM_LOCK: i64 = 0x0D17_0001;

/// The directory outbox over the table [`crate::PostgresRepositoryDirectory`] writes.
pub struct PostgresDirectoryOutbox {
    pool: PgPool,
}

impl PostgresDirectoryOutbox {
    /// An outbox over `database`.
    #[must_use]
    pub fn new(database: &Database) -> Self {
        Self {
            pool: database.pool.clone(),
        }
    }
}

fn backend(error: impl ToString) -> DirectoryOutboxError {
    DirectoryOutboxError::Backend(error.to_string())
}

fn kind(text: &str) -> Result<ChangeKind, DirectoryOutboxError> {
    match text {
        "created" => Ok(ChangeKind::Created),
        "deleted" => Ok(ChangeKind::Deleted),
        other => Err(backend(format!("a stored change kind {other:?} is unknown"))),
    }
}

type Row = (i64, String, String, String, i64);

fn change(row: Row) -> Result<RepositoryChange, DirectoryOutboxError> {
    let (repository, organisation, name, kind_text, sequence) = row;
    Ok(RepositoryChange {
        organisation: organisation.parse().map_err(backend)?,
        name: name.parse().map_err(backend)?,
        repository: conversion::repository(repository).map_err(backend)?,
        kind: kind(&kind_text)?,
        sequence: u64::try_from(sequence)
            .map_err(|_| backend(format!("a stored sequence {sequence} is negative")))?,
    })
}

impl DirectoryOutbox for PostgresDirectoryOutbox {
    async fn claim(
        &self,
        repository: Option<RepositoryId>,
        now: i64,
        lease: i64,
    ) -> Result<Vec<RepositoryChange>, DirectoryOutboxError> {
        let repository = repository
            .map(conversion::repository_column)
            .transpose()
            .map_err(backend)?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        sqlx::query("select pg_advisory_xact_lock($1)")
            .bind(CLAIM_LOCK)
            .execute(&mut *transaction)
            .await
            .map_err(backend)?;
        let rows = sqlx::query_as::<_, Row>(
            "with target as ( \
                 select repository from directory_outbox \
                 where $1::bigint is null or repository = $1 \
                 group by repository \
                 having bool_and(coalesce(lease_expires_at, $2) <= $2) \
                 order by min(id) limit 1) \
             update directory_outbox set lease_expires_at = $2 + $3 \
             where repository in (select repository from target) \
             returning repository, organisation, name, kind, sequence",
        )
        .bind(repository)
        .bind(now)
        .bind(lease)
        .fetch_all(&mut *transaction)
        .await
        .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        let mut changes = rows
            .into_iter()
            .map(change)
            .collect::<Result<Vec<_>, _>>()?;
        changes.sort_by_key(|change| change.sequence);
        Ok(changes)
    }

    async fn release(
        &self,
        repository: RepositoryId,
        until: i64,
    ) -> Result<(), DirectoryOutboxError> {
        let repository = conversion::repository_column(repository).map_err(backend)?;
        sqlx::query("update directory_outbox set lease_expires_at = $2 where repository = $1")
            .bind(repository)
            .bind(until)
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn delete(
        &self,
        repository: RepositoryId,
        sequence: u64,
    ) -> Result<(), DirectoryOutboxError> {
        let repository = conversion::repository_column(repository).map_err(backend)?;
        let sequence = conversion::number_column(sequence, "sequence").map_err(backend)?;
        sqlx::query("delete from directory_outbox where repository = $1 and sequence = $2")
            .bind(repository)
            .bind(sequence)
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }
}
