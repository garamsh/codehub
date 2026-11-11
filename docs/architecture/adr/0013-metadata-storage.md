# ADR 0013: PostgreSQL holds the metadata, the changeset log and the refs, and blobs stay in the object store

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0003 left the storage of source control open, and `docs/architecture/source-control.md` listed the metadata database and the changeset log storage as open questions, recommending PostgreSQL behind a port for the single-machine initial deployment and the log as a table in it. ADR 0012 made moving a ref a compare-and-swap on that ref alone, separate from creating the changeset. The user confirmed storage in the design session recorded in issue #12 (decision 8).

What Anthropic's account of Scion (https://www.anthropic.com/engineering/redesigning-source-control) states about its storage:

- Moving a ref is a compare-and-swap on a single Spanner row.
- Servers compute derived data and write the results back to Spanner.
- Changesets point at the compressed file contents in blob storage, and servers read from Spanner or cloud storage as needed.
- The servers are essentially large caches, and each tier of the system does not require long-lived state.

Alternatives not taken:

- SQLite behind a single writer.
- A distributed SQL database now.
- The changeset log as append-only segments in the object store, with an index in the database.

Issue #12 states no reason against them beyond the decision itself, which keeps the distributed option open for later.

## Decision

- PostgreSQL holds the metadata, the changeset log as a table, and the refs, each reached through a port.
- Blobs are in the S3-compatible object store.
- The ports name no PostgreSQL feature, so that a distributed SQL store can serve them later without the application changing.

## Consequences

- The changeset log and the refs are in one database, and a ref's compare-and-swap is a conditional update of one row there.
- PostgreSQL is a dependency of every operation that reads or changes the log or a ref, and its availability bounds the system's.
- Write throughput on the log and on `main` is bounded by one PostgreSQL primary until the move to distributed SQL.
- A port whose operations rely on a feature only PostgreSQL provides makes the later move a change to the application, so each port's operations are checked against that when it is written.
- A workspace's data spans two stores, the database and the object store, so the export bundle of ADR 0005 is read from both.
- Where derived data is stored, and how PostgreSQL is run, are left open in `docs/architecture/source-control.md` §Open questions.

## Signs the decision was wrong

- Landings queue on the PostgreSQL primary — lock waits or write latency on the log table or the `main` row grow with the number of concurrent agents — before the target of hundreds of agents is reached.
- The changeset log table grows to a size where reading history or appending slows measurably, and partitioning it is needed before distributed SQL would have been.
- A port turns out to depend on PostgreSQL behaviour, such as a transaction isolation level or an advisory lock, so a distributed SQL adapter cannot serve it without the application changing.
- The first real deployment already needs more than one region or more than one writer, which one PostgreSQL primary cannot provide.
- Backups or restores disagree between the database and the object store often enough that keeping the log beside the blobs would have been simpler.
