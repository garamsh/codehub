use app::domain::address::Address;
use app::domain::draft::Instance;
use app::domain::job::CacheRef;
use app::domain::path::{Path, Prefix};
use app::domain::repository::RepositoryId;
use app::port::driven::cache_refs::{CacheRefs, CacheRefsError};
use sqlx::PgPool;

use crate::conversion;
use crate::database::Database;
use crate::statements::Statements;

/// The cache refs in PostgreSQL.
pub struct PostgresCacheRefs {
    pool: PgPool,
    statements: Statements,
}

impl PostgresCacheRefs {
    /// Cache refs over `database`.
    #[must_use]
    pub fn new(database: &Database) -> Self {
        Self {
            pool: database.pool.clone(),
            statements: database.statements,
        }
    }
}

fn backend(error: impl ToString) -> CacheRefsError {
    CacheRefsError::Backend(error.to_string())
}

fn instance_column(instance: Instance) -> Result<i64, CacheRefsError> {
    i64::try_from(instance.get()).map_err(|_| {
        backend(format!(
            "workspace instance {instance} is outside the stored range"
        ))
    })
}

#[async_trait::async_trait]
impl CacheRefs for PostgresCacheRefs {
    async fn cache_ref(
        &self,
        repository: RepositoryId,
        folder: &Prefix,
        path: &Path,
    ) -> Result<Option<CacheRef>, CacheRefsError> {
        let row = self
            .statements
            .query_as::<(Vec<u8>, Vec<u8>)>(
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
                self.statements
                    .query(
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
                self.statements
                    .query(
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

    async fn workspace_cache_ref(
        &self,
        repository: RepositoryId,
        instance: Instance,
        path: &Path,
    ) -> Result<Option<Address>, CacheRefsError> {
        let tree = self
            .statements
            .query_scalar::<Vec<u8>>(
                "select tree from workspace_cache_refs \
             where repository = $1 and instance = $2 and cache_path = $3",
            )
            .bind(conversion::repository_column(repository).map_err(backend)?)
            .bind(instance_column(instance)?)
            .bind(conversion::path_column(path))
            .fetch_optional(&self.pool)
            .await
            .map_err(backend)?;
        tree.map(conversion::address).transpose().map_err(backend)
    }

    async fn create_workspace_cache_ref(
        &self,
        repository: RepositoryId,
        instance: Instance,
        path: &Path,
        tree: Address,
    ) -> Result<bool, CacheRefsError> {
        let created = self
            .statements
            .query(
                "insert into workspace_cache_refs (repository, instance, cache_path, tree) \
             values ($1, $2, $3, $4) on conflict do nothing",
            )
            .bind(conversion::repository_column(repository).map_err(backend)?)
            .bind(instance_column(instance)?)
            .bind(conversion::path_column(path))
            .bind(tree.digest().as_slice())
            .execute(&self.pool)
            .await
            .map_err(backend)?
            .rows_affected();
        Ok(created == 1)
    }

    async fn move_workspace_cache_ref(
        &self,
        repository: RepositoryId,
        instance: Instance,
        path: &Path,
        from: Address,
        to: Address,
    ) -> Result<bool, CacheRefsError> {
        let moved = self
            .statements
            .query(
                "update workspace_cache_refs set tree = $4, updated_at = now() \
             where repository = $1 and instance = $2 and cache_path = $3 and tree = $5",
            )
            .bind(conversion::repository_column(repository).map_err(backend)?)
            .bind(instance_column(instance)?)
            .bind(conversion::path_column(path))
            .bind(to.digest().as_slice())
            .bind(from.digest().as_slice())
            .execute(&self.pool)
            .await
            .map_err(backend)?
            .rows_affected();
        Ok(moved == 1)
    }

    async fn remove_workspace_cache_refs(
        &self,
        repository: RepositoryId,
        instance: Instance,
    ) -> Result<(), CacheRefsError> {
        self.statements
            .query("delete from workspace_cache_refs where repository = $1 and instance = $2")
            .bind(conversion::repository_column(repository).map_err(backend)?)
            .bind(instance_column(instance)?)
            .execute(&self.pool)
            .await
            .map_err(backend)?;
        Ok(())
    }
}
