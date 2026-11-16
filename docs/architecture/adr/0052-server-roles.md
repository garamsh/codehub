# ADR 0052: the server binary runs as one role, chosen by a subcommand

> Status: accepted
> Date: 2026-11-16

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The server is one process that migrates the schema, serves every route of the API, serves the runner's routes, and delivers the directory outbox (ADR 0022, ADR 0044). One process is the right unit for a developer and for the checks, but not for a deployment on Kubernetes: the API of users and agents and the runner API have different load and different callers, delivery is background work that should run once or a few times and not once per replica, and a migration should run once per version, as a step of its own, not at the start of every replica. Goal #273 puts the server in a Helm chart, and #274 asks the binary to be runnable as each of those parts.

Two ways to name the part a process runs were considered and not taken:

- **A `--target` flag**, as Loki and Mimir use. One flat option set serves every target, so a target takes options it does not use, `--help` lists all of them for each, and nothing refuses an option set meant for another target.
- **Separate services with RPC between them**, such as a delivery service the API calls. It adds a protocol, an address and a failure mode to every pair of parts, where PostgreSQL and the object store already are the state every part reads and writes, and the outbox (ADR 0044) is already a queue in PostgreSQL.

## Decision

`codehub` takes a subcommand naming one role, and the roles talk to each other only through PostgreSQL and the object store:

- `api` serves `/v1`, except `/v1/runner/`; `runner-api` serves `/v1/runner/`; `worker` delivers the directory outbox; `migrate` applies the migrations and exits 0; `all` does every one of those in one process.
- With no subcommand `codehub` is `all`: the command line is read as `all` followed by the arguments, so every existing invocation, test and Make target keeps its behaviour. `--help`, `-h`, `--version` and `-V` alone are left to the top-level command.
- Each role is a clap subcommand of its own with its own options, built from option groups it includes (listening address, database, object store, identity, authorisation, the AuthZEN client, landing, runner, directory, delivery wait), so a role refuses an option it does not take (exit code 2) and its `--help` lists only its own. No option is renamed and every option keeps its variable. An environment variable of an option the role does not take is ignored, which is what lets one environment serve every role of a deployment.
- `api`, `runner-api` and `worker` do not migrate; `migrate` and `all` do. A role that does not migrate answers `/readyz` with 503, naming the schema, while the database's latest applied migration is older than the latest one the binary embeds. A newer schema is accepted, so the old binary keeps serving while a rolling upgrade has migrated first.
- Every role that serves, `worker` included, answers `/healthz` and `/readyz` on its listening address. The worker's readiness asks PostgreSQL and the schema, and no object store, which it does not hold. The routers are split in `crates/adapter-http` as `api_router`, `runner_router` and `health_router` beside the unchanged `router`.
- The schema check is `Database::schema_is_current` in `crates/adapter-postgres`: the highest successfully applied version in `_sqlx_migrations` is at least the highest embedded one; no such table is not current.
- Choices that follow from the option split: the worker takes the database, the listening address, the AuthZEN client and the directory options and no identity or object-store option, since delivery to garam needs a token client and a URL and nothing else; `runner-api` takes no directory option and its changes go nowhere (`()`), since it creates no repository; roles that do not serve runners or wait on a creation use fixed unused defaults for those settings, which they never read.

## Consequences

- A deployment runs `migrate` once as a step, then any number of `api` and `runner-api` replicas and a `worker`, each configured with only what it uses and each probed by `/readyz`; the one-process deployment is unchanged.
- A role's options are enforced by its command line, not by a check at run time, and `docs/guide/deployment.md` carries which roles take each option; a test compares that table with each role's `--help`.
- `/readyz` of a role that does not migrate is a gate on the schema, so a new version's replicas wait for `migrate` and an orchestrator does not send them traffic early.
- Each role connects to PostgreSQL on its own, so the pool-size rule of ADR 0037 sums over every replica of every role.
- Adds one public method to `crates/adapter-postgres`, so the change reaches a crate the issue did not name; the pool is private to it.
- Rules out running a delivery on a role that serves: `all` relays the outbox in its own process and `api` does not, so an `api` deployment with no `worker` delivers only what a creation's own wait delivers (ADR 0044) and leaves the rest in the outbox.

## Signs the decision was wrong

- A deployment needs a setting on a role that does not take it, so the environment of one role is not the other's and a role's option set is widened or duplicated.
- The schema check blocks readiness for a migration that does not change what the binary reads, which a rolling upgrade should have survived, so a finer compatibility than the latest version is wanted.
- Roles need to call each other directly, and PostgreSQL polling is too slow or too costly for it.
- A reader mistakes `api` for a process that delivers, or `worker` for one that serves routes, because the role names do not say what each does.
