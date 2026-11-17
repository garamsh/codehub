//! Finding the writable primary among the hosts of a connection URL (ADR 0054).
//!
//! sqlx parses one host and no `target_session_attrs`, so a libpq-style URL naming several hosts
//! is split here into one single-host URL per host, and each is parsed by sqlx.

use std::str::FromStr;
use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgConnection};
use sqlx::{ConnectOptions, Connection};

use crate::statements::Statements;

/// How long one host may take to connect and answer whether it is in recovery.
pub(crate) const HOST_TIMEOUT: Duration = Duration::from_secs(5);

/// The connect options of each host `url` names, in the order named, with a statement cache of
/// `statement_cache` statements per connection and, unless `statement_timeout` is zero, the
/// statement timeout set at connection start.
///
/// # Errors
///
/// A description where `url` is not a PostgreSQL URL, or `target_session_attrs` is a value other
/// than `read-write`: codehub writes, so a host that is not writable is never wanted.
pub(crate) fn hosts(
    url: &str,
    statement_cache: usize,
    statement_timeout: Duration,
) -> Result<Vec<PgConnectOptions>, String> {
    let (scheme, rest) = url
        .split_once("://")
        .filter(|(scheme, _)| matches!(*scheme, "postgres" | "postgresql"))
        .ok_or("not a postgres:// or postgresql:// URL")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (userinfo, hosts) = match authority.rfind('@') {
        Some(at) => authority.split_at(at + 1),
        None => ("", authority),
    };
    let tail = without_session_attrs(tail)?;
    hosts
        .split(',')
        .map(|host| {
            let single = format!("{scheme}://{userinfo}{host}{tail}");
            let options = PgConnectOptions::from_str(&single)
                .map_err(|error| error.to_string())?
                .statement_cache_capacity(statement_cache);
            Ok(if statement_timeout.is_zero() {
                options
            } else {
                options.options([(
                    "statement_timeout",
                    statement_timeout.as_millis().to_string(),
                )])
            })
        })
        .collect()
}

/// `tail`, the part of a URL from its path on, without a `target_session_attrs` parameter.
fn without_session_attrs(tail: &str) -> Result<String, String> {
    let Some((path, query)) = tail.split_once('?') else {
        return Ok(tail.to_owned());
    };
    let (query, fragment) = query.split_once('#').unwrap_or((query, ""));
    let mut kept = Vec::new();
    for parameter in query.split('&') {
        match parameter.split_once('=') {
            Some(("target_session_attrs", "read-write")) => {}
            Some(("target_session_attrs", other)) => {
                return Err(format!(
                    "target_session_attrs={other} is not supported, only read-write"
                ));
            }
            _ if parameter.is_empty() => {}
            _ => kept.push(parameter),
        }
    }
    let mut rebuilt = path.to_owned();
    if !kept.is_empty() {
        rebuilt.push('?');
        rebuilt.push_str(&kept.join("&"));
    }
    if !fragment.is_empty() {
        rebuilt.push('#');
        rebuilt.push_str(fragment);
    }
    Ok(rebuilt)
}

/// Whether the server `connection` reaches is in recovery, which is a standby.
pub(crate) async fn in_recovery(
    statements: Statements,
    connection: &mut PgConnection,
) -> Result<bool, sqlx::Error> {
    statements
        .query_scalar("select pg_is_in_recovery()")
        .fetch_one(connection)
        .await
}

/// Tries each of `hosts` in order and returns the options of the first that connects and is not in
/// recovery, with the connection made to it.
///
/// # Errors
///
/// A description of why each host was refused, where none is a writable primary.
pub(crate) async fn find(
    statements: Statements,
    hosts: &[PgConnectOptions],
) -> Result<(PgConnectOptions, PgConnection), String> {
    let mut refused = Vec::new();
    for options in hosts {
        let host = format!("{}:{}", options.get_host(), options.get_port());
        let attempt = tokio::time::timeout(HOST_TIMEOUT, async {
            let mut connection = options.connect().await?;
            let standby = in_recovery(statements, &mut connection).await?;
            Ok::<_, sqlx::Error>((standby, connection))
        })
        .await;
        match attempt {
            Ok(Ok((false, connection))) => return Ok((options.clone(), connection)),
            Ok(Ok((true, _))) => refused.push(format!("{host} is a standby")),
            Ok(Err(error)) => refused.push(format!("{host}: {error}")),
            Err(_) => refused.push(format!("{host} did not answer in time")),
        }
    }
    Err(format!(
        "no writable primary among the hosts: {}",
        refused.join("; ")
    ))
}

/// Closes `connection` without waiting longer than [`HOST_TIMEOUT`].
pub(crate) async fn close(connection: PgConnection) {
    let _ = tokio::time::timeout(HOST_TIMEOUT, connection.close()).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(url: &str) -> Result<Vec<String>, String> {
        hosts(url, 100, Duration::ZERO).map(|hosts| {
            hosts
                .iter()
                .map(|options| format!("{}:{}", options.get_host(), options.get_port()))
                .collect()
        })
    }

    #[test]
    fn a_url_names_its_hosts_in_order() {
        assert_eq!(
            named(
                "postgres://u:p@a.example:5433,b.example,[::1]:7/db?target_session_attrs=read-write"
            )
            .unwrap(),
            ["a.example:5433", "b.example:5432", "[::1]:7"]
        );
        assert_eq!(
            named("postgresql://localhost/db").unwrap(),
            ["localhost:5432"]
        );
    }

    #[test]
    fn a_password_with_an_at_sign_or_comma_belongs_to_the_userinfo() {
        let options = hosts("postgres://u:p%2Cq%40r@a,b/db", 1, Duration::ZERO).unwrap();
        assert_eq!(options.len(), 2);
        assert_eq!(options[1].get_host(), "b");
        assert_eq!(options[1].get_username(), "u");
    }

    #[test]
    fn other_parameters_are_kept_and_other_session_attributes_refused() {
        let options = hosts(
            "postgres://a,b/db?target_session_attrs=read-write&application_name=x",
            1,
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(options[0].get_application_name(), Some("x"));
        let refused = named("postgres://a,b/db?target_session_attrs=any").unwrap_err();
        assert!(refused.contains("read-write"), "{refused}");
        assert!(named("mysql://a/db").is_err());
    }

    #[test]
    fn the_statement_timeout_is_a_startup_option_unless_it_is_zero() {
        let with = hosts("postgres://a/db", 1, Duration::from_secs(3)).unwrap();
        assert_eq!(with[0].get_options(), Some("-c statement_timeout=3000"));
        let without = hosts("postgres://a/db", 1, Duration::ZERO).unwrap();
        assert_eq!(without[0].get_options(), None);
    }
}
