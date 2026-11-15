use app::domain::address::Address;
use app::domain::job::CacheRef;
use app::domain::path::{Path, Prefix};
use app::domain::repository::RepositoryId;
use app::port::driven::cache_refs::{CacheRefs, CacheRefsError};
use sqlx::PgPool;

use crate::conversion;
use crate::database::Database;

/// The cache refs in PostgreSQL.
pub struct PostgresCacheRefs {
    pool: PgPool,
}

impl PostgresCacheRefs {
    /// Cache refs over `database`.
    #[must_use]
    pub fn new(database: &Database) -> Self {
        Self {
            pool: database.pool.clone(),
        }
    }
}

fn backend(error: impl ToString) -> CacheRefsError {
    CacheRefsError::Backend(error.to_string())
}

#[async_trait::async_trait]
impl CacheRefs for PostgresCacheRefs {
    async fn cache_ref(
        &self,
        repository: RepositoryId,
        folder: &Prefix,
        path: &Path,
    ) -> Result<Option<CacheRef>, CacheRefsError> {
        let row = sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
            "select tree, changeset from cache_refs \
             where repository = $1 and folder = $2 and cache_path = $3",
        )
        .bind(conversion::repository_column(repository).map_err(backend)?)
        .bind(conversion::prefix_column(folder))
        .bind(conversion::path_column(path))
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?;
        row.map(|(tree, changeset)| {
            Ok::<_, String>(CacheRef {
                tree: conversion::address(tree)?,
                changeset: conversion::address(changeset)?,
            })
        })
        .transpose()
        .map_err(backend)
    }

    async fn advance_cache_ref(
        &self,
        repository: RepositoryId,
        folder: &Prefix,
        path: &Path,
        from: Option<Address>,
        to: CacheRef,
    ) -> Result<bool, CacheRefsError> {
        let repository = conversion::repository_column(repository).map_err(backend)?;
        let folder = conversion::prefix_column(folder);
        let path = conversion::path_column(path);
        let tree = to.tree.digest().as_slice();
        let changeset = to.changeset.digest().as_slice();
        let moved = match from {
            None => {
                sqlx::query(
                    "insert into cache_refs (repository, folder, cache_path, tree, changeset) \
                     values ($1, $2, $3, $4, $5) on conflict do nothing",
                )
                .bind(repository)
                .bind(folder)
                .bind(path)
                .bind(tree)
                .bind(changeset)
                .execute(&self.pool)
                .await
            }
            Some(from) => {
                sqlx::query(
                    "update cache_refs set tree = $4, changeset = $5, updated_at = now() \
                     where repository = $1 and folder = $2 and cache_path = $3 and changeset = $6",
                )
                .bind(repository)
                .bind(folder)
                .bind(path)
                .bind(tree)
                .bind(changeset)
                .bind(from.digest().as_slice())
                .execute(&self.pool)
                .await
            }
        }
        .map_err(backend)?
        .rows_affected();
        Ok(moved == 1)
    }
}
