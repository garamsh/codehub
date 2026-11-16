use std::collections::BTreeMap;

use crate::conversion;
use crate::database::Database;
use crate::retry;
use app::domain::address::Address;
use app::domain::encoding::EncodeError;
use app::domain::file_version::FileVersionNode;
use app::domain::path::Path;
use app::domain::repository::RepositoryId;
use app::domain::tree::Tree;
use app::port::driven::derived_data::{DerivedData, DerivedDataError};

/// Derived data in PostgreSQL. Each value is a function of its changeset, so recording one again
/// leaves the same rows.
pub struct PostgresDerivedData {
    database: Database,
}

impl PostgresDerivedData {
    /// Derived data over `database`.
    #[must_use]
    pub fn new(database: &Database) -> Self {
        Self {
            database: database.clone(),
        }
    }
}

fn backend(error: impl ToString) -> DerivedDataError {
    DerivedDataError::Backend(error.to_string())
}

fn repository_column(repository: RepositoryId) -> Result<i64, DerivedDataError> {
    conversion::repository_column(repository).map_err(DerivedDataError::Backend)
}

/// Two `bytea[]` columns of equal length: addresses and encodings.
type Columns = (Vec<Vec<u8>>, Vec<Vec<u8>>);

/// The addresses and encodings of `objects`.
fn columns<T>(
    objects: BTreeMap<Address, T>,
    encode: impl Fn(&T) -> Result<Vec<u8>, EncodeError>,
) -> Result<Columns, DerivedDataError> {
    let mut addresses = Vec::with_capacity(objects.len());
    let mut encodings = Vec::with_capacity(objects.len());
    for (address, object) in &objects {
        addresses.push(address.digest().to_vec());
        encodings.push(encode(object).map_err(backend)?);
    }
    Ok((addresses, encodings))
}

#[async_trait::async_trait]
impl DerivedData for PostgresDerivedData {
    async fn put_trees(
        &self,
        repository: RepositoryId,
        changeset: Address,
        root: Address,
        trees: BTreeMap<Address, Tree>,
    ) -> Result<(), DerivedDataError> {
        let repository = repository_column(repository)?;
        let (addresses, encodings) = columns(trees, Tree::encode)?;
        let (addresses, encodings) = (&addresses, &encodings);
        retry::idempotent(|| async move {
            let mut transaction = self.database.pool.begin().await?;
            self.database.statements.query(
                "insert into trees (repository, address, encoding) \
                 select $1, address, encoding from unnest($2::bytea[], $3::bytea[]) as t (address, encoding) \
                 on conflict (repository, address) do nothing",
            )
            .bind(repository)
            .bind(addresses.clone())
            .bind(encodings.clone())
            .execute(&mut *transaction)
            .await?;
            self.database.statements.query(
                "insert into tree_roots (repository, changeset, root) values ($1, $2, $3) \
                 on conflict (repository, changeset) do update set root = excluded.root",
            )
            .bind(repository)
            .bind(changeset.digest().as_slice())
            .bind(root.digest().as_slice())
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await
        })
        .await
        .map_err(backend)
    }

    async fn root(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> Result<Address, DerivedDataError> {
        let repository = repository_column(repository)?;
        let root = self
            .database
            .immutable(|pool| async move {
                self.database
                    .statements
                    .query_scalar::<Vec<u8>>(
                        "select root from tree_roots where repository = $1 and changeset = $2",
                    )
                    .bind(repository)
                    .bind(changeset.digest().as_slice())
                    .fetch_optional(&pool)
                    .await
            })
            .await
            .map_err(backend)?
            .ok_or(DerivedDataError::NotFound(changeset))?;
        conversion::address(root).map_err(DerivedDataError::Backend)
    }

    async fn tree(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<Tree, DerivedDataError> {
        let repository = repository_column(repository)?;
        let encoding = self
            .database
            .immutable(|pool| async move {
                self.database
                    .statements
                    .query_scalar::<Vec<u8>>(
                        "select encoding from trees where repository = $1 and address = $2",
                    )
                    .bind(repository)
                    .bind(address.digest().as_slice())
                    .fetch_optional(&pool)
                    .await
            })
            .await
            .map_err(backend)?
            .ok_or(DerivedDataError::NotFound(address))?;
        Tree::decode(&encoding).map_err(backend)
    }

    async fn put_file_versions(
        &self,
        repository: RepositoryId,
        changeset: Address,
        value: BTreeMap<Path, Address>,
        nodes: BTreeMap<Address, FileVersionNode>,
    ) -> Result<(), DerivedDataError> {
        let repository = repository_column(repository)?;
        let (addresses, encodings) = columns(nodes, FileVersionNode::encode)?;
        let (paths, targets): (Vec<_>, Vec<_>) = value
            .iter()
            .map(|(path, node)| (conversion::path_column(path), node.digest().to_vec()))
            .unzip();
        let (addresses, encodings, paths, targets) = (&addresses, &encodings, &paths, &targets);
        retry::idempotent(|| async move {
            let mut transaction = self.database.pool.begin().await?;
            self.database.statements.query(
                "insert into file_version_nodes (repository, address, encoding) \
                 select $1, address, encoding from unnest($2::bytea[], $3::bytea[]) as t (address, encoding) \
                 on conflict (repository, address) do nothing",
            )
            .bind(repository)
            .bind(addresses.clone())
            .bind(encodings.clone())
            .execute(&mut *transaction)
            .await?;
            self.database.statements.query(
                "insert into file_version_sets (repository, changeset) values ($1, $2) \
                 on conflict (repository, changeset) do nothing",
            )
            .bind(repository)
            .bind(changeset.digest().as_slice())
            .execute(&mut *transaction)
            .await?;
            self.database.statements.query(
                "insert into file_version_entries (repository, changeset, path, node) \
                 select $1, $2, path, node from unnest($3::bytea[], $4::bytea[]) as t (path, node) \
                 on conflict (repository, changeset, path) do update set node = excluded.node",
            )
            .bind(repository)
            .bind(changeset.digest().as_slice())
            .bind(paths.clone())
            .bind(targets.clone())
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await
        })
        .await
        .map_err(backend)
    }

    async fn file_versions(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> Result<Option<BTreeMap<Path, Address>>, DerivedDataError> {
        // One statement reads the marker and the entries from one snapshot; a set with no entries
        // yields its marker row alone, with null columns.
        let repository = repository_column(repository)?;
        let rows = self
            .database
            .immutable(|pool| async move {
                let rows = self
                    .database
                    .statements
                    .query_as::<(Option<Vec<u8>>, Option<Vec<u8>>)>(
                        "select e.path, e.node from file_version_sets s \
                     left join file_version_entries e \
                     on e.repository = s.repository and e.changeset = s.changeset \
                     where s.repository = $1 and s.changeset = $2",
                    )
                    .bind(repository)
                    .bind(changeset.digest().as_slice())
                    .fetch_all(&pool)
                    .await?;
                Ok((!rows.is_empty()).then_some(rows))
            })
            .await
            .map_err(backend)?;
        let Some(rows) = rows else {
            return Ok(None);
        };
        let mut versions = BTreeMap::new();
        for row in rows {
            if let (Some(path), Some(node)) = row {
                versions.insert(
                    conversion::path(&path).map_err(DerivedDataError::Backend)?,
                    conversion::address(node).map_err(DerivedDataError::Backend)?,
                );
            }
        }
        Ok(Some(versions))
    }

    async fn file_version_node(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<FileVersionNode, DerivedDataError> {
        let repository = repository_column(repository)?;
        let encoding = self
            .database
            .immutable(|pool| async move {
                self.database.statements.query_scalar::<Vec<u8>>(
                    "select encoding from file_version_nodes where repository = $1 and address = $2",
                )
                .bind(repository)
                .bind(address.digest().as_slice())
                .fetch_optional(&pool)
                .await
            })
            .await
            .map_err(backend)?
            .ok_or(DerivedDataError::NotFound(address))?;
        FileVersionNode::decode(&encoding).map_err(backend)
    }
}
