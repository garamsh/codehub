# ADR 0001: The system is structured as a hexagon

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The same subject matter, version control of a workspace's changesets and objects, is reached through several interfaces (the `ch` CLI, an MCP server, runners) and stored and executed through technologies expected to change independently of it: object storage, a metadata database, executor drivers, an authorisation decision service. The tests that matter exercise the subject matter with no technology attached. The candidates were the hexagonal, layered and domain structures.

## Decision

The system is a hexagon: an application that names no technology, and adapters around it that reach it only through ports. Its rules are in `docs/architecture/structure.md`.

## Consequences

- Every conversation with the outside is written twice, as a port in the application's terms and an adapter translating to the technology's.
- Following one request end to end means reading the adapter, the port and the application behind it.
- Adding or replacing a technology touches an adapter and the composition site, not the application.
- The inside has no internal limit; whether it is partitioned further is not decided here.

## Signs the decision was wrong

- A technology's vocabulary appears in the application's own signatures — a framework type, a column name, a status code.
- Adding or replacing a technology still changes the application.
- Most ports have one adapter, and the tests substitute none of them.
- The same rule is implemented in two adapters, and a fix has to be made in each.
- Ports are added and dropped with every feature, because they were drawn per use case rather than per conversation.
- Code cannot be found by subject matter, because everything the system decides sits in one undivided inside.
