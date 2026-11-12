# ADR 0040: The move to distributed SQL is triggered by a measured saturation of the PostgreSQL primary, after the cheaper steps

> Status: accepted
> Date: 2026-11-13

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0013 keeps the metadata, the changeset log and the refs in PostgreSQL. It keeps the ports free of PostgreSQL features so that a distributed SQL store can serve them later, and accepts that write throughput on the log and on `main` is bounded by one primary until then. `source-control.md` left open what measurement triggers the move. Goal issue #91 asks for it to be decided.

The scale baseline (`docs/scale-baseline.md`) ran 200 agents against two server replicas sharing one PostgreSQL on a 12-core machine. PostgreSQL's CPU was sampled every 10 seconds:
- a median of about 2.0 cores, at most 2.7, at `595f4c3`, while lost swaps went back to the client;
- a median of about 0.9 cores, at most 3.3, once the server retried them (ADR 0036) and landings of one repository passed a gate in each process (ADR 0039), while the scenarios landed 3 to 7 times as many changesets.

The primary had most of the machine to spare. The limits measured were in the server: lost swaps, decision-point calls and the connection pool.

Alternatives not taken:

- **Move now.** Nothing measured is bounded by the primary, and a distributed store adds latency to every write and an operational system to run.
- **A trigger by agent count**, such as "beyond N agents". The cost per agent depends on the scenario: `read-heavy` and `external` differ by an order of magnitude in operations per second. The count does not measure what the move would relieve.
- **Leave the question open.** A deployment would then meet saturation with no agreed reading of it, and would argue the move under pressure.

## Decision

The move to a distributed SQL store is proposed, as a new ADR choosing the store, when all three of these hold:

1. **The primary is the bottleneck.** At the deployment's peak landing load, either:
   - the PostgreSQL primary sustains more than 70 % of its CPU or of its storage I/O for 15 minutes; or
   - the server's append-and-swap of a landing has a p99 above 1 second.

   Either holds while the server processes are below 70 % of their own CPU, so adding replicas would not help.
2. **The cheaper steps are taken.** These are:
   - the primary is on the largest single machine the operator runs;
   - connections go through an external pooler where ADR 0037 calls for one;
   - the changeset log is partitioned, if ADR 0013's sign about its size has appeared.

   Condition 1 still holds after them.
3. **It is reproduced.** `crates/loadtest`, scaled to the deployment's agent count and mix of scenarios, shows condition 1 on a copy of the deployment's configuration.

Until then, no adapter for a distributed store is built. A PostgreSQL feature used inside an adapter, such as `skip locked` in the job queue, stays behind its port and names nothing in the port.

## Consequences

- The question of ADR 0013's timing has a test that an operator can run and a reviewer can check.
- The baseline's sampling of PostgreSQL CPU becomes part of `make loadtest`'s report, so that condition 1 can be read from the same harness; until it does, it is sampled beside the harness, as here.
- A deployment that reaches the trigger runs on one primary in the meantime, with the latency that implies.

## Signs the decision was wrong

- A deployment saturates its primary without condition 1 having been observable beforehand, for example through a sudden lock-contention collapse rather than a gradual rise.
- The move is needed for a reason other than write throughput, such as multi-region availability, and the trigger delays it.
- An adapter has come to depend on PostgreSQL behaviour that a distributed store cannot give, so that, when the trigger fires, the move needs application changes ADR 0013 meant to avoid.
