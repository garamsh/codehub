# ADR 0054: the server runs against a primary that fails over, a pooler or a read replica

> Status: accepted
> Date: 2026-11-16

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The server held one pool to one PostgreSQL host, took a connection from it for as long as a request needed one, and failed a request on the first error. That is right for the developer and the checks and wrong for the deployments of goal #273: a managed PostgreSQL moves its primary during maintenance and a failure, a pooler such as PgBouncer or RDS Proxy sits in front of the primary, and a read replica can carry reads that a lag cannot make wrong. A request that waits for a connection without a bound holds its caller; a statement without a bound holds a connection; a failover that needs a restart of every replica turns a minute of unavailability into an incident.

Alternatives not taken:

- **A library that handles failover** (a driver with `target_session_attrs`, or a proxy in the deployment). sqlx parses one host and no session attributes, and a proxy is the deployer's choice, not something the binary can require. The binary finds the primary itself and also runs behind a proxy.
- **A replica for every read.** A ref, a draft, a queue, a grant and a run change after they are written, and a replica that lags answers with an older value; a stale ref lets a landing be built on a `main` that has moved, and a stale queue hands a job twice. Only data keyed by its own address, which no later write changes, is safe to read behind a lag.
- **Retrying every failed statement.** A commit whose outcome is unknown, retried, applies twice or answers with the wrong result.

## Decision

**Finding the primary.** `CODEHUB_POSTGRES_URL` may name several hosts, `postgres://user:password@a,b/db?target_session_attrs=read-write`, as libpq does. The server asks each host, in order, `pg_is_in_recovery()` and uses the first that answers false, each bounded to 5 seconds, since a host that is loaded answers slowly and a host that is gone is skipped at the bound. Only `read-write` is accepted and the parameter may be left out; codehub writes, so a host that is not writable is never wanted. A connection the pool makes to a standby is refused. A supervisor task polls the server it last found every second; where that server is a standby or does not answer, it looks for the primary among the hosts again, points the pool at it, and the pool closes every connection made before the move when it is next used. The polling is not a feature of a driver and costs one statement a second per process. `/readyz` answers 503 while no host is a writable primary.

**Retries.** A statement is retried at most 3 times, 50 ms, then 100 ms, then 200 ms apart, in two classes:

- `idempotent`, for a read and for a write that leaves the same rows however often it runs: retried on every transient error, which is a pool timeout, a serialization failure or a deadlock (SQLSTATE 40001, 40P01), a refusal on a standby (25006), a server shutdown (57P01, 57P02, 57P03), a connection error (class 08) and a broken connection.
- `definite`, for any other statement: retried only where the error proves nothing was applied, which is a pool timeout, 40001, 40P01 and 25006.

A commit of unknown outcome is in neither class and is not retried blindly. Applied: the changeset log's reads and `append`, which does nothing on a conflict; derived data's writes, each whole transaction, since they insert by address; the claim of a job and of a run, which reads whether anything waits before it opens a transaction or locks a row; and the reservation of a revert, which is `definite`, because run again after an unknown outcome it would answer that the reservation was already held. The other adapters are not wrapped; their statements are not idempotent in their answers, and a request that fails there fails with its error, as before.

**The compare-and-swap of `main`.** A swap that lost its connection after the statement was sent is settled, not retried. This is the rule #291 built and is recorded here because failover makes it matter:

1. Every swap statement carries a tag of its attempt, `/*swap <pid>-<nanos>-<n>*/`, and is sent like every other statement, named where the statement cache is on (its text is new for each attempt, so the cache evicts it in turn) and unnamed with cache 0, since an unnamed statement behind a transaction pooler fails (below). The tag is not a backend's pid, because a transaction pooler moves statements between backends.
2. Where the outcome is unknown (a connection I/O error, SQLSTATE 57P01, 57P02, 57P03, class 08, or a crashed worker), every backend that is not idle and whose `pg_stat_activity.query` holds the tag is ended with `pg_terminate_backend`, again until none is left, for at most 5 seconds. A backend that is idle ran its statement to its end, and one that was ended rolled an unfinished transaction back, so the statement cannot commit afterwards. A spent bound settles as a backend error.
3. The swap won if and only if the new changeset is `main` or is on `main`'s first-parent chain, read to at most 1000 changesets. If `main` is still at the expected value the result is a backend error; any other value is `RefMoved`.

After a failover the lookup finds no backend, since the old primary's are gone, and the read of `main` on the new primary answers: a swap the standby had replicated is won, one it had not is a backend error with `main` unchanged.

Two limits of the rule are known:

- A statement that was sent and that no backend has started cannot be found by its tag and may still commit after the read. Direct to the server the window is the time between the bytes arriving and the backend starting the statement, which is microseconds; behind a pooler it is the time the statement waits for a server connection, bounded by the pooler's `query_wait_timeout`. Closing it needs a fence row that every swap reads, which adds a statement to every landing to guard against a window of microseconds direct to the server. Not closed. The result is a landing reported as failed that appears on `main` afterwards.
- The lookup fails open where the role cannot see other backends' `query`: `track_activities` off, or backends of another role. The role codehub connects with needs `pg_signal_backend` (or to own the backends, which it does when every connection uses one role), and behind a pooler the pooler must connect to PostgreSQL as that role.

**Timeouts.** Waiting for a connection ends after `CODEHUB_POSTGRES_ACQUIRE_TIMEOUT`, 5 seconds by default (1 to 3600), so with up to 4 attempts a request spends about 20 seconds at most before it fails, which covers a typical failover; and a statement ends after `CODEHUB_POSTGRES_STATEMENT_TIMEOUT`, 30 seconds by default, 0 for none. The statement timeout is sent as the startup option `-c statement_timeout=<ms>`, so it holds for every statement of a connection and for a migration only where it is not 0; the `migrate` role sets 0.

**A pooler.** In sqlx-postgres 0.9 a statement is named (`sqlx_s_<n>`) whenever its query is persistent, which every query is unless it says otherwise, and the connection closes a named statement only when its statement cache evicts it (`executor.rs:31-37`, `:184`). A statement sent unnamed is sent as Parse, Describe and Sync, and then, in a second round trip, as Bind, Execute and Sync (`executor.rs:62`, `:278`).

- Behind a transaction-mode pooler the pooler must track the prepared statements of its clients and the cache must be 1 or more: PgBouncer 1.21 or later with `max_prepared_statements` above 0 and at least the cache. `CODEHUB_POSTGRES_STATEMENT_CACHE` is 100 by default. `compose.yaml`'s pooler profile sets `max_prepared_statements` to 100, and the pooler end-to-end test lands and reads through it. The same test with `max_prepared_statements` 0 fails: the pooler may give the server connection to another client between the two rounds of an unnamed statement, and the server answered `unnamed prepared statement does not exist` and `bind message supplies 0 parameters, but prepared statement "" requires 4`; with the cache on and no tracking it answered `prepared statement "sqlx_s_1" already exists`.
- A cache of 0 sends every statement unnamed, so a connection holds no prepared statement, and it does not work behind a transaction-mode pooler for the reason above. It is for a direct connection or a session-mode pooler. The flag is set in one place, `Statements`, which every `query`, `query_as` and `query_scalar` of the crate goes through, so a statement added later cannot forget it. A test fails where `sqlx::query` is called anywhere else in the crate's sources, and another that a connection of a pool with cache 0 holds no row of `pg_prepared_statements` after the adapters ran, which fails when one call site is made persistent again.
- RDS Proxy: codehub requires a pooler that keeps the named statements of a client consistent. How RDS Proxy treats the named statements of a client, and whether it pins the connection to one, is not verified here: no AWS documentation was read for this ADR, and a deployment behind it runs the pooler test against it before relying on it.
- Two alternatives were rejected. Wrapping every autocommit statement in a transaction when the cache is 0 would pin the backend across both rounds, and adds two round trips to every statement to support a pooler mode PgBouncer itself has superseded. Patching or replacing sqlx is outside #281 and against #278 C1.
- The migrations of sqlx send named statements of their own, so `CODEHUB_POSTGRES_MIGRATE_URL` names a direct connection to the primary for the `migrate` role and for `all`; a migration also takes a session advisory lock, which a transaction pooler does not keep. Unset, migration uses `CODEHUB_POSTGRES_URL`.
- Behind PgBouncer set `ignore_startup_parameters=options`, so the startup option above is ignored and not refused; a statement timeout then belongs on the role (`alter role ... set statement_timeout`) or in the pooler's `query_timeout`.

**A read replica.** `CODEHUB_POSTGRES_READ_URL` names one host. A changeset read by its id and the derived data (`root`, `tree`, `file_version_node`, `file_versions`) are read from it first; a miss, an error and a replica that is down are read from the primary again, so a lag never answers wrong, and a hit is served because the data is keyed by its own address and cannot change. A ref, a draft, a queue, a grant and a run never touch it, and neither does `first_parent_chain`: a replica that lags holds a part of a chain, and a part of a chain is an error, not a miss. The replica pool is made on first use. The issue names "objects' metadata" as replica-eligible; no PostgreSQL table holds it, since objects live in the object store, so the eligible data is what is listed above. The API and runner API roles read the replica; `worker` and `migrate` do not.

**Deployment of the failover.** Zero loss of a landing the server acknowledged needs synchronous replication, `synchronous_standby_names` on the primary: with asynchronous replication a commit the primary acknowledged and the standby had not received is lost with the primary, which no client of PostgreSQL can prevent. `compose.yaml`'s profile `pg-modes` runs it that way for the test.

## Consequences

- A deployment lists the hosts of its cluster in one URL, or fronts them with a proxy and lists one; it sets the migrate URL and, behind a transaction-mode pooler, a pooler that tracks prepared statements with the default cache, and gives codehub's role `pg_signal_backend` where the swap settles under a pooler.
- A failover shows as 503 from `/readyz` and as requests waiting up to about 20 seconds, not as errors that need a restart; an orchestrator holding traffic back while `/readyz` is 503 makes the gap invisible to callers.
- The sum of pools (ADR 0037) now includes the replica pool, which is the same size as the primary's, made lazily; a deployment sums it as a pool against the replica's own `max_connections`.
- The supervisor adds one connection per process, held for the probe, outside the pool.
- Reaches `crates/adapter-postgres` beyond the connection: the changeset log, derived data and the two claims, each minimally, since all the changes they depend on (#274, #266, #280, #291, #263) are merged.
- Adds `tokio`'s `time` feature to `crates/adapter-postgres`; no crate is added.

## Signs the decision was wrong

- A swap settles as won where it was not, or as lost where it won, in a failover test; the rule of the three steps then needs the fence row.
- The supervisor's second of polling makes a deployment's failover longer than its target, or its statements a measurable load; a push from the server (a listener on the replication state) or a longer period is then wanted.
- A deployment needs a replica read of data this ADR keeps on the primary, and the lag there is shown to be harmless, or a hit rate of the replica too low to be worth its connections.
- A retry hides an error that an operator needed to see, so a bound is spent without a trace in the logs.
- A managed pooler is found that refuses the startup option `options`; a statement timeout on the role alone is then wanted, and the option dropped.
