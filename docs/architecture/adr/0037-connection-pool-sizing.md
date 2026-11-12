# ADR 0037: Each server process pools 40 PostgreSQL connections by default, and the sum stays below the server's limit

> Status: accepted
> Date: 2026-11-12

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0022 pools connections in process with a configurable size, and defers an external pooler until the connection count is a problem. `CODEHUB_POSTGRES_POOL_SIZE` defaults to 10.

The scale baseline (`docs/scale-baseline.md`) swept the pool size at 200 agents against two replicas. Against pool 10, pool 40 raised `read-heavy` throughput from 165.8 to 215.6 operations per second, and lowered `disjoint` land p50 from 370 ms to 257 ms. Pool 4 was slower than both.

PostgreSQL's `max_connections` defaults to 100.

Alternatives not taken:

- **Keep 10.** The sweep shows requests waiting for a connection at 200 agents.
- **A larger default, such as 100.** Two replicas would then exceed PostgreSQL's default `max_connections`, and the sweep did not measure beyond 40.
- **An external pooler now.** ADR 0022's reason stands: another process to run, before the count is a problem.

## Decision

- The default pool size is 40 connections per server process.
- An operator keeps the sum of pool sizes over all server processes below PostgreSQL's `max_connections`, less the connections other clients use.
- When the processes a deployment needs cannot fit their pools under that limit, an external pooler is introduced, as ADR 0022 foresees.

## Consequences

- Two replicas at the default take 80 connections, which fits under PostgreSQL's default limit of 100.
- A third replica at the default does not fit under that limit: the operator lowers the pool size or raises `max_connections`.

## Signs the decision was wrong

- PostgreSQL CPU or lock waits rise with the pool size while throughput does not, so the larger pool only moves the queue into the database.
- Deployments routinely run more replicas than fit at the default.
