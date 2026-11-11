//! Conversions between the application's values and what the tables store.

use app::domain::address::Address;
use app::domain::path::{Component, Path, Prefix};
use app::domain::repository::RepositoryId;
use app::port::driven::membership::{Access, Role};

/// The `bigint` a repository is stored under.
pub(crate) fn repository_column(repository: RepositoryId) -> Result<i64, String> {
    i64::try_from(repository.value())
        .map_err(|_| format!("repository {repository} is outside the stored range"))
}

/// The address whose digest is the `bytea` `bytes`.
pub(crate) fn address(bytes: Vec<u8>) -> Result<Address, String> {
    <[u8; 32]>::try_from(bytes)
        .map(Address::from_digest)
        .map_err(|bytes| format!("a stored address has {} bytes, not 32", bytes.len()))
}

/// The `bytea` of a path: its components joined by `/`.
pub(crate) fn path_column(path: &Path) -> Vec<u8> {
    join(path.components())
}

/// The path stored as `bytes`.
pub(crate) fn path(bytes: &[u8]) -> Result<Path, String> {
    let components = split(bytes)?;
    Path::new(components).map_err(|error| format!("a stored path is invalid: {error}"))
}

/// The `bytea` of a prefix: its components joined by `/`; empty for the whole repository.
pub(crate) fn prefix_column(prefix: &Prefix) -> Vec<u8> {
    join(prefix.components())
}

/// The prefix stored as `bytes`.
pub(crate) fn prefix(bytes: &[u8]) -> Result<Prefix, String> {
    if bytes.is_empty() {
        Ok(Prefix::whole())
    } else {
        split(bytes).map(Prefix::new)
    }
}

/// The `role` column of `role`.
pub(crate) const fn role_column(role: Role) -> &'static str {
    match role {
        Role::Admin => "admin",
        Role::Member => "member",
    }
}

/// The role stored as `text`.
pub(crate) fn role(text: &str) -> Result<Role, String> {
    match text {
        "admin" => Ok(Role::Admin),
        "member" => Ok(Role::Member),
        other => Err(format!("a stored role is unknown: {other:?}")),
    }
}

/// The `access` column of `access`.
pub(crate) const fn access_column(access: Access) -> &'static str {
    match access {
        Access::Read => "read",
        Access::Write => "write",
    }
}

/// The access stored as `text`.
pub(crate) fn access(text: &str) -> Result<Access, String> {
    match text {
        "read" => Ok(Access::Read),
        "write" => Ok(Access::Write),
        other => Err(format!("a stored access is unknown: {other:?}")),
    }
}

fn join(components: &[Component]) -> Vec<u8> {
    components
        .iter()
        .map(Component::as_bytes)
        .collect::<Vec<_>>()
        .join(&b'/')
}

fn split(bytes: &[u8]) -> Result<Vec<Component>, String> {
    bytes
        .split(|byte| *byte == b'/')
        .map(|part| {
            Component::new(part.to_vec())
                .map_err(|error| format!("a stored component is invalid: {error}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_and_a_prefix_round_trip_through_their_columns() {
        let path: Path = "src/a b/main.rs".parse().unwrap();
        assert_eq!(path_column(&path), b"src/a b/main.rs");
        assert_eq!(super::path(&path_column(&path)), Ok(path.clone()));
        let prefix = Prefix::from(path);
        assert_eq!(super::prefix(&prefix_column(&prefix)), Ok(prefix));
        assert_eq!(prefix_column(&Prefix::whole()), Vec::<u8>::new());
        assert_eq!(super::prefix(b""), Ok(Prefix::whole()));
    }

    #[test]
    fn a_role_and_an_access_round_trip_through_their_columns() {
        for held in [Role::Admin, Role::Member] {
            assert_eq!(role(role_column(held)), Ok(held));
        }
        for held in [Access::Read, Access::Write] {
            assert_eq!(access(access_column(held)), Ok(held));
        }
        assert!(role("owner").is_err());
        assert!(access("admin").is_err());
    }

    #[test]
    fn an_address_round_trips_and_a_short_column_is_refused() {
        let address_of = Address::of(b"x");
        assert_eq!(address(address_of.digest().to_vec()), Ok(address_of));
        assert!(address(vec![0; 31]).is_err());
    }

    #[test]
    fn a_repository_beyond_the_stored_range_is_refused() {
        assert_eq!(repository_column(RepositoryId::new(7)), Ok(7));
        assert!(repository_column(RepositoryId::new(u64::MAX)).is_err());
    }
}
