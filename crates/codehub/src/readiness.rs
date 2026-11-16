//! The readiness probe: a writable PostgreSQL primary, the schema it holds and the object store each
//! answer within [`LIMIT`].

use std::sync::Arc;
use std::time::Duration;

use adapter_http::{NotReady, Readiness};
use adapter_postgres::{Database, DatabaseError};
use app::domain::address::Address;
use app::domain::repository::RepositoryId;
use app::port::driven::object_store::ObjectStore;
use app::port::driven::repository_directory::RepositoryDirectory;
use tokio::time::timeout;

/// How long each dependency has to answer.
const LIMIT: Duration = Duration::from_secs(2);

/// What the database must be for this binary: a writable primary with the schema it was built for.
#[async_trait::async_trait]
pub(crate) trait Schema: Send + Sync {
    /// Whether every migration this binary embeds has been applied; a newer schema is current.
    async fn is_current(&self) -> Result<bool, DatabaseError>;

    /// Whether the server answering is a writable primary and not a standby (ADR 0054).
    async fn is_writable(&self) -> Result<bool, DatabaseError>;
}

#[async_trait::async_trait]
impl Schema for Database {
    async fn is_current(&self) -> Result<bool, DatabaseError> {
        self.schema_is_current().await
    }

    async fn is_writable(&self) -> Result<bool, DatabaseError> {
        Database::is_writable(self).await
    }
}

/// Asks the directory, which PostgreSQL holds, the schema and the object store a question each. A
/// role that keeps no objects has none to ask.
pub(crate) struct Probe {
    pub(crate) directory: Arc<dyn RepositoryDirectory>,
    pub(crate) schema: Arc<dyn Schema>,
    pub(crate) objects: Option<Arc<dyn ObjectStore>>,
}

#[async_trait::async_trait]
impl Readiness for Probe {
    async fn check(&self) -> Result<(), NotReady> {
        // Neither repository 0 nor the empty object need exist: a question answered "none" is
        // an answer.
        let probed = RepositoryId::new(0);
        let (writable, schema, database, objects) = tokio::join!(
            timeout(LIMIT, self.schema.is_writable()),
            timeout(LIMIT, self.schema.is_current()),
            timeout(LIMIT, self.directory.names_of(probed)),
            async {
                match &self.objects {
                    Some(objects) => {
                        Some(timeout(LIMIT, objects.exists(probed, Address::of(b""))).await)
                    }
                    None => None,
                }
            },
        );
        // A standby, or no server at all, cannot take a landing, so it is the reason given first.
        match writable {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => return Err(NotReady("no writable PostgreSQL primary answers")),
            _ => return Err(NotReady("PostgreSQL did not answer")),
        }
        // A database that has not been migrated cannot answer the directory's question, so the
        // schema is the reason given first.
        match schema {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => {
                return Err(NotReady("the database schema is older than this binary"));
            }
            _ => return Err(NotReady("PostgreSQL did not answer")),
        }
        if !matches!(database, Ok(Ok(_))) {
            return Err(NotReady("PostgreSQL did not answer"));
        }
        if objects.is_some_and(|answer| !matches!(answer, Ok(Ok(_)))) {
            return Err(NotReady("the object store did not answer"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app::domain::name::{OrganisationName, RepositoryName};
    use app::port::driven::object_store::{Grant, ObjectStoreError};
    use app::port::driven::repository_directory::DirectoryError;

    /// How a stand-in answers.
    #[derive(Clone, Copy)]
    enum Answer {
        Ok,
        Fails,
        Hangs,
    }

    async fn answer<T>(how: Answer, value: T, failure: impl FnOnce() -> T) -> T {
        match how {
            Answer::Ok => value,
            Answer::Fails => failure(),
            Answer::Hangs => std::future::pending().await,
        }
    }

    struct Directory(Answer);

    #[async_trait::async_trait]
    impl RepositoryDirectory for Directory {
        async fn create_organisation(&self, _: &OrganisationName) -> Result<(), DirectoryError> {
            Ok(())
        }

        async fn create_repository(
            &self,
            _: &OrganisationName,
            _: &RepositoryName,
        ) -> Result<RepositoryId, DirectoryError> {
            Ok(RepositoryId::new(1))
        }

        async fn find_repository(
            &self,
            _: &OrganisationName,
            _: &RepositoryName,
        ) -> Result<RepositoryId, DirectoryError> {
            Ok(RepositoryId::new(1))
        }

        async fn names_of(
            &self,
            _: RepositoryId,
        ) -> Result<Option<(OrganisationName, RepositoryName)>, DirectoryError> {
            answer(self.0, Ok(None), || {
                Err(DirectoryError::Backend("down".to_owned()))
            })
            .await
        }
    }

    struct Objects(Answer);

    #[async_trait::async_trait]
    impl ObjectStore for Objects {
        async fn put(&self, _: RepositoryId, _: Vec<u8>) -> Result<Address, ObjectStoreError> {
            Ok(Address::of(b""))
        }

        async fn get(
            &self,
            _: RepositoryId,
            address: Address,
        ) -> Result<Vec<u8>, ObjectStoreError> {
            Err(ObjectStoreError::NotFound(address))
        }

        async fn size(&self, _: RepositoryId, address: Address) -> Result<u64, ObjectStoreError> {
            Err(ObjectStoreError::NotFound(address))
        }

        async fn exists(&self, _: RepositoryId, _: Address) -> Result<bool, ObjectStoreError> {
            answer(self.0, Ok(false), || {
                Err(ObjectStoreError::Backend("down".to_owned()))
            })
            .await
        }

        async fn upload_grant(
            &self,
            _: RepositoryId,
            address: Address,
            _: u64,
        ) -> Result<Grant, ObjectStoreError> {
            Err(ObjectStoreError::NotFound(address))
        }

        async fn download_grant(
            &self,
            _: RepositoryId,
            address: Address,
        ) -> Result<Grant, ObjectStoreError> {
            Err(ObjectStoreError::NotFound(address))
        }
    }

    /// A schema that is current, older than the binary, or whose question fails, on a server that
    /// is a writable primary, a standby, or whose question fails.
    struct Schemas {
        current: Result<bool, ()>,
        writable: Result<bool, ()>,
    }

    #[async_trait::async_trait]
    impl Schema for Schemas {
        async fn is_current(&self) -> Result<bool, DatabaseError> {
            self.current
                .map_err(|()| DatabaseError::Connect("down".to_owned()))
        }

        async fn is_writable(&self) -> Result<bool, DatabaseError> {
            self.writable
                .map_err(|()| DatabaseError::Connect("down".to_owned()))
        }
    }

    async fn check_schema(
        schema: Result<bool, ()>,
        objects: Option<Answer>,
    ) -> Result<(), NotReady> {
        let probe = Probe {
            directory: Arc::new(Directory(Answer::Ok)),
            schema: Arc::new(Schemas {
                current: schema,
                writable: Ok(true),
            }),
            objects: objects.map(|answer| Arc::new(Objects(answer)) as Arc<dyn ObjectStore>),
        };
        probe.check().await
    }

    async fn check(database: Answer, objects: Answer) -> Result<(), NotReady> {
        let probe = Probe {
            directory: Arc::new(Directory(database)),
            schema: Arc::new(Schemas {
                current: Ok(true),
                writable: Ok(true),
            }),
            objects: Some(Arc::new(Objects(objects))),
        };
        probe.check().await
    }

    #[tokio::test(start_paused = true)]
    async fn is_not_ready_where_the_schema_is_older_than_the_binary() {
        assert_eq!(check_schema(Ok(true), Some(Answer::Ok)).await, Ok(()));
        assert_eq!(
            check_schema(Ok(false), Some(Answer::Ok)).await,
            Err(NotReady("the database schema is older than this binary"))
        );
        assert_eq!(
            check_schema(Err(()), Some(Answer::Ok)).await,
            Err(NotReady("PostgreSQL did not answer"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn is_not_ready_while_no_writable_primary_answers() {
        let check_writable = |writable| async move {
            Probe {
                directory: Arc::new(Directory(Answer::Ok)),
                schema: Arc::new(Schemas {
                    current: Ok(true),
                    writable,
                }),
                objects: None,
            }
            .check()
            .await
        };
        assert_eq!(check_writable(Ok(true)).await, Ok(()));
        assert_eq!(
            check_writable(Ok(false)).await,
            Err(NotReady("no writable PostgreSQL primary answers"))
        );
        assert_eq!(
            check_writable(Err(())).await,
            Err(NotReady("PostgreSQL did not answer"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn asks_no_object_store_of_a_role_that_has_none() {
        assert_eq!(check_schema(Ok(true), None).await, Ok(()));
        assert_eq!(
            check_schema(Ok(true), Some(Answer::Fails)).await,
            Err(NotReady("the object store did not answer"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn is_ready_where_both_answer() {
        assert_eq!(check(Answer::Ok, Answer::Ok).await, Ok(()));
    }

    #[tokio::test(start_paused = true)]
    async fn is_not_ready_where_the_database_fails_or_hangs() {
        let postgres = Err(NotReady("PostgreSQL did not answer"));
        assert_eq!(check(Answer::Fails, Answer::Ok).await, postgres);
        assert_eq!(check(Answer::Hangs, Answer::Ok).await, postgres);
    }

    #[tokio::test(start_paused = true)]
    async fn is_not_ready_where_the_object_store_fails_or_hangs() {
        let objects = Err(NotReady("the object store did not answer"));
        assert_eq!(check(Answer::Ok, Answer::Fails).await, objects);
        assert_eq!(check(Answer::Ok, Answer::Hangs).await, objects);
    }

    #[tokio::test(start_paused = true)]
    async fn gives_a_dependency_two_seconds_to_answer() {
        let started = tokio::time::Instant::now();
        assert!(check(Answer::Hangs, Answer::Ok).await.is_err());
        assert_eq!(started.elapsed(), Duration::from_secs(2));
    }
}
