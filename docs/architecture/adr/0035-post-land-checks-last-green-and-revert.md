# ADR 0035: Post-land checks are configured in the repository, move `last-green` and revert a blocking failure

> Status: accepted
> Date: 2026-11-12

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0012 lands on `main` before checks run, and keeps `last-green` as the ref that CI and deployment base on, because a broken `main` contaminates work until the revert. `source-control.md` left open which checks run, who configures them, what moves `last-green`, when a revert is automatic and how a revert with dependants works. Goal issue #91 (decision 4) settled these with the user:
- checks are in `.codehub/checks.toml`, each with a command, labels and a `blocking` flag;
- `last-green` moves when every blocking check passes;
- a blocking failure reverts automatically, as a new changeset.

Alternatives not taken:
- **A server-wide set of checks.** Checks differ per repository and change with the code they check.
- **Every failing check reverts.** A flaky or advisory check would revert good work.
- **A person or agent decides each revert.** `main` stays broken until someone acts, which is what ADR 0012's `last-green` argument guards against.
- **Refusing the revert while later changesets depend on the change.** The broken change stays on `main` for as long as dependants keep landing.

## Decision

**Configuration**
- A repository's checks are the file `.codehub/checks.toml` in the tree of the changeset being checked.
- Each check has a name, a command, labels (default none), `blocking` (default true) and a timeout (default 600 seconds).
- A changeset without the file has no checks.
- A file that does not parse counts as one failed blocking check named for the file.

**Queuing**
- Each landing on `main` queues one job per check of the landed changeset. Each job is routed by the check's labels (ADR 0034).
- A job's result is passed, failed, timed out or cancelled. A runner that could not run the job, for example because a download failed, reports it as failed.

**`last-green`**
- `last-green` is a ref (ADR 0012), moved by compare-and-swap.
- It moves to a changeset on `main` when every blocking check of that changeset has passed, and only forward: to a changeset later on `main` than the one it names.
- A changeset with no blocking checks counts as passed when it lands.

**Revert**
- When a blocking check of a changeset fails or times out, the server lands a new changeset on `main` that undoes that changeset's delta. The landing merge (ADR 0012) carries later non-overlapping changes over.
- A changeset is reverted at most once, however many of its blocking checks fail.
- A revert made this way is not itself reverted automatically.
- A revert that conflicts with later changes is not landed. The conflict is recorded with the check result, for a person or agent to resolve.
- A cancelled check, and any non-blocking check whatever its result, reverts nothing.
- The revert changeset is authored by the server, and its message names the reverted changeset and the failed check.

**Reading**
- A principal who may read the repository may read a changeset's check results and `last-green`.

## Consequences

- A repository's checks travel with its code: a change and the checks it needs land together, and a revert restores the earlier checks with the earlier code.
- `main` stays a moving head for agents. `last-green` lags it by the checks' run time, and deployment bases on `last-green`.
- A failing blocking check undoes work without a person present. Its author learns of it from the check result and the revert changeset.
- Queuing follows the compare-and-swap through a separate port. A server that stops between the two leaves a landing with no checks, and so with no move of `last-green` until a later landing passes. Recovering those landings is an open question.

## Signs the decision was wrong

- Reverts conflict often, because agents build on a change before its checks finish, and `main` stays broken anyway.
- Flaky blocking checks revert good changes often enough that authors mark checks non-blocking to avoid it.
- Teams want checks the code under test must not change, such as a required security scan, which configuration in the repository cannot enforce.
- Landings whose checks were never queued are observed in practice.
