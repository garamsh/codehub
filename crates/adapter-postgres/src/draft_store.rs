use std::collections::BTreeMap;

use app::domain::address::Address;
use app::domain::changeset::Change;
use app::domain::draft::{Draft, DraftId, Edit};
use app::domain::file::{File, FileKind};
use app::domain::path::Path;
use app::domain::repository::RepositoryId;
use app::port::driven::draft_store::{DraftStore, DraftStoreError};
use sqlx::PgPool;

use crate::conversion;
use crate::database::Database;

/// The draft store in PostgreSQL.
pub struct PostgresDraftStore {
    pool: PgPool,
}

impl PostgresDraftStore {
    /// A draft store over `database`.
    #[must_use]
    pub fn new(database: &Database) -> Self {
        Self {
            pool: database.pool.clone(),
        }
    }
}

fn backend(error: impl ToString) -> DraftStoreError {
    DraftStoreError::Backend(error.to_string())
}

fn repository_column(repository: RepositoryId) -> Result<i64, DraftStoreError> {
    conversion::repository_column(repository).map_err(DraftStoreError::Backend)
}

/// A change as its `kind` and `content` columns.
fn change_columns(change: Change) -> (&'static str, Option<Vec<u8>>) {
    match change {
        Change::Deletion => ("deletion", None),
        Change::File(File {
            kind: FileKind::Regular,
            content,
        }) => ("regular", Some(content.digest().to_vec())),
        Change::File(File {
            kind: FileKind::Executable,
            content,
        }) => ("executable", Some(content.digest().to_vec())),
    }
}

fn change(kind: &str, content: Option<Vec<u8>>) -> Result<Change, String> {
    let kind = match kind {
        "deletion" => return Ok(Change::Deletion),
        "regular" => FileKind::Regular,
        "executable" => FileKind::Executable,
        other => return Err(format!("a stored change has the unknown kind {other:?}")),
    };
    let content = content.ok_or_else(|| "a stored file change has no content".to_owned())?;
    Ok(Change::File(File {
        kind,
        content: conversion::address(content)?,
    }))
}

/// A draft's base and description, and one of its changes' path, kind and content; the change is
/// null for a draft with none.
type Row = (
    Option<Vec<u8>>,
    String,
    Option<Vec<u8>>,
    Option<String>,
    Option<Vec<u8>>,
);

impl DraftStore for PostgresDraftStore {
    async fn stage(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        base: Option<Address>,
        path: Path,
        change: Change,
    ) -> Result<(), DraftStoreError> {
        let (kind, content) = change_columns(change);
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        // The no-op update returns the row of a draft already held, with its own base.
        let id = sqlx::query_scalar::<_, i64>(
            "insert into drafts (repository, issuer, subject, name, base) \
             values ($1, $2, $3, $4, $5) \
             on conflict (repository, issuer, subject, name) do update set name = drafts.name \
             returning id",
        )
        .bind(repository_column(repository)?)
        .bind(draft.principal.issuer())
        .bind(draft.principal.subject())
        .bind(draft.name.as_str())
        .bind(base.as_ref().map(|base| base.digest().as_slice()))
        .fetch_one(&mut *transaction)
        .await
        .map_err(backend)?;
        sqlx::query(
            "insert into draft_changes (draft, path, kind, content) values ($1, $2, $3, $4) \
             on conflict (draft, path) do update \
             set kind = excluded.kind, content = excluded.content",
        )
        .bind(id)
        .bind(conversion::path_column(&path))
        .bind(kind)
        .bind(content)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
        transaction.commit().await.map_err(backend)
    }

    async fn open(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        base: Option<Address>,
    ) -> Result<Draft, DraftStoreError> {
        // The no-op update keeps the base of a draft already held.
        sqlx::query(
            "insert into drafts (repository, issuer, subject, name, base) \
             values ($1, $2, $3, $4, $5) \
             on conflict (repository, issuer, subject, name) do update set name = drafts.name",
        )
        .bind(repository_column(repository)?)
        .bind(draft.principal.issuer())
        .bind(draft.principal.subject())
        .bind(draft.name.as_str())
        .bind(base.as_ref().map(|base| base.digest().as_slice()))
        .execute(&self.pool)
        .await
        .map_err(backend)?;
        self.get(repository, draft).await
    }

    async fn describe(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        description: String,
    ) -> Result<(), DraftStoreError> {
        let described = sqlx::query(
            "update drafts set description = $5 \
             where repository = $1 and issuer = $2 and subject = $3 and name = $4",
        )
        .bind(repository_column(repository)?)
        .bind(draft.principal.issuer())
        .bind(draft.principal.subject())
        .bind(draft.name.as_str())
        .bind(description)
        .execute(&self.pool)
        .await
        .map_err(backend)?
        .rows_affected();
        if described == 0 {
            return Err(DraftStoreError::NotFound(draft.clone()));
        }
        Ok(())
    }

    async fn record(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        edits: BTreeMap<Path, Edit>,
    ) -> Result<(), DraftStoreError> {
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let id = sqlx::query_scalar::<_, i64>(
            "select id from drafts \
             where repository = $1 and issuer = $2 and subject = $3 and name = $4 \
             for update",
        )
        .bind(repository_column(repository)?)
        .bind(draft.principal.issuer())
        .bind(draft.principal.subject())
        .bind(draft.name.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(backend)?
        .ok_or_else(|| DraftStoreError::NotFound(draft.clone()))?;
        for (path, edit) in edits {
            match edit {
                Edit::Record(change) => {
                    let (kind, content) = change_columns(change);
                    sqlx::query(
                        "insert into draft_changes (draft, path, kind, content) \
                         values ($1, $2, $3, $4) \
                         on conflict (draft, path) do update \
                         set kind = excluded.kind, content = excluded.content",
                    )
                    .bind(id)
                    .bind(conversion::path_column(&path))
                    .bind(kind)
                    .bind(content)
                    .execute(&mut *transaction)
                    .await
                    .map_err(backend)?;
                }
                Edit::Restore => {
                    sqlx::query("delete from draft_changes where draft = $1 and path = $2")
                        .bind(id)
                        .bind(conversion::path_column(&path))
                        .execute(&mut *transaction)
                        .await
                        .map_err(backend)?;
                }
            }
        }
        transaction.commit().await.map_err(backend)
    }

    async fn get(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
    ) -> Result<Draft, DraftStoreError> {
        // One statement, so the base and the changes are from one snapshot; a draft with no
        // changes yields its row alone, with null change columns.
        let rows = sqlx::query_as::<_, Row>(
            "select d.base, d.description, c.path, c.kind, c.content from drafts d \
             left join draft_changes c on c.draft = d.id \
             where d.repository = $1 and d.issuer = $2 and d.subject = $3 and d.name = $4",
        )
        .bind(repository_column(repository)?)
        .bind(draft.principal.issuer())
        .bind(draft.principal.subject())
        .bind(draft.name.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        let Some((base, description, ..)) = rows.first() else {
            return Err(DraftStoreError::NotFound(draft.clone()));
        };
        let base = base
            .clone()
            .map(conversion::address)
            .transpose()
            .map_err(DraftStoreError::Backend)?;
        let description = description.clone();
        let mut changes = BTreeMap::new();
        for (_, _, path, kind, content) in rows {
            if let (Some(path), Some(kind)) = (path, kind) {
                changes.insert(
                    conversion::path(&path).map_err(DraftStoreError::Backend)?,
                    self::change(&kind, content).map_err(DraftStoreError::Backend)?,
                );
            }
        }
        Ok(Draft {
            base,
            description,
            changes,
        })
    }

    async fn remove(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
    ) -> Result<(), DraftStoreError> {
        let removed = sqlx::query(
            "delete from drafts \
             where repository = $1 and issuer = $2 and subject = $3 and name = $4",
        )
        .bind(repository_column(repository)?)
        .bind(draft.principal.issuer())
        .bind(draft.principal.subject())
        .bind(draft.name.as_str())
        .execute(&self.pool)
        .await
        .map_err(backend)?
        .rows_affected();
        if removed == 0 {
            return Err(DraftStoreError::NotFound(draft.clone()));
        }
        Ok(())
    }
}
