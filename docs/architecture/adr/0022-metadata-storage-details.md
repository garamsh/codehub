# ADR 0022: Metadata lives in one shared PostgreSQL schema, migrated forward-only at server start

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0013 chose PostgreSQL for the metadata, the changeset log and the refs, and left open where derived data and drafts are stored and how PostgreSQL is run (`docs/architecture/source-control.md`). ADR 0021 adds grants to the data PostgreSQL holds.

Alternatives not taken:

- A schema or database per organisation. It isolates tenants, but migrations then run once per tenant and the number of schemas grows with the number of organisations.
- Derived data in the object store as content-addressed objects. Trees are read on every checkout and are small, so a store round trip per read costs more than a row.
- An external pooler in front of PostgreSQL. It is another process to run before a connection count is a problem.
- Migration tooling run by an operator before start. A server that starts against a schema it did not migrate can run against the wrong one.

Fact checked on 2026-11-11 against https://www.postgresql.org/support/versioning/: the current stable major version of PostgreSQL is 18 (latest minor 18.6, released 25 September 2025, supported until 14 November 2030); PostgreSQL 19 is at release candidate stage.

## Decision

- PostgreSQL holds the repository directory, the changeset log, refs, drafts, derived data and the default authorisation adapter's grants, in one schema shared by all organisations, with a repository identifier on every row.
- The minimum major version is 18. The server takes a connection URL, so PostgreSQL may be run by the project or consumed as a managed service.
- Migrations are forward-only SQL files in `crates/adapter-postgres`, applied at server start by that crate's migration runner.
- The server pools connections in process, with a configurable pool size.
- A landing that lost the compare-and-swap leaves its changeset and derived rows. They are content-addressed, so a retry reuses them. Their removal waits for the retention decision, which is still open.
- A draft is identified by repository, principal and draft name.

## Consequences

- One migration history serves every organisation, and isolation between organisations rests on the repository identifier in every query.
- Every table and every query carries the repository identifier; a query without it is a defect.
- The server cannot start against a database it cannot migrate.
- A rollback is a new forward migration, not a down script.
- Rows from lost landings accumulate until retention is decided.
- A principal has one draft per name per repository, so an agent working in parallel names its drafts.

## Signs the decision was wrong

- A query without the repository identifier is found, or a tenant sees another's rows.
- A tenant needs its own database for isolation, residency or deletion and the shared schema cannot give it.
- Forward-only migrations block a deployment that needed a rollback.
- The in-process pool is exhausted or spread across so many server processes that PostgreSQL's connection limit is reached, and an external pooler is introduced.
- Rows of lost landings grow faster than the retention decision arrives.

## Errata

- 2026-11-12: "with a repository identifier on every row" is true of the rows below the repository directory, not of every row. `organisations`, `members` and `repositories` (`crates/adapter-postgres/migrations/0001_initial.sql`) carry none: they are the directory itself, or keyed by organisation. Found by the architecture review of goal issue #91 (finding D5). The decision stands.
