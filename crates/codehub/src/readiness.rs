//! The readiness probe: PostgreSQL and the object store each answer within [`LIMIT`].

use std::time::Duration;

use adapter_http::{NotReady, Readiness};
use app::domain::address::Address;
use app::domain::repository::RepositoryId;
use app::port::driven::object_store::ObjectStore;
use app::port::driven::repository_directory::RepositoryDirectory;
use tokio::time::timeout;

/// How long each dependency has to answer.
const LIMIT: Duration = Duration::from_secs(2);

/// Asks the directory, which PostgreSQL holds, and the object store a question each.
pub(crate) struct Probe<D: 'static, O: 'static> {
    pub(crate) directory: &'static D,
    pub(crate) objects: &'static O,
}

impl<D, O> Readiness for Probe<D, O>
where
    D: RepositoryDirectory + Send + Sync,
    O: ObjectStore + Send + Sync,
{
    async fn check(&self) -> Result<(), NotReady> {
        // Neither repository 0 nor the empty object need exist: a question answered "none" is
        // an answer.
        let probed = RepositoryId::new(0);
        let (database, objects) = tokio::join!(
            timeout(LIMIT, self.directory.names_of(probed)),
            timeout(LIMIT, self.objects.exists(probed, Address::of(b""))),
        );
        if !matches!(database, Ok(Ok(_))) {
            return Err(NotReady("PostgreSQL did not answer"));
        }
        if !matches!(objects, Ok(Ok(_))) {
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

    async fn check(database: Answer, objects: Answer) -> Result<(), NotReady> {
        let probe = Probe {
            directory: Box::leak(Box::new(Directory(database))),
            objects: Box::leak(Box::new(Objects(objects))),
        };
        probe.check().await
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
