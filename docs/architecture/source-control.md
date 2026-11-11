# Source control

How versions of a workspace's content are stored, changed, landed and queried.

## Current decisions

- **The server holds the truth.** Clients (`ch`, the MCP server, runners) keep at most a bounded cache of objects. Everything that creates a changeset, rebases, lands or reverts runs on the server (ADR 0003).
- **Objects are content-addressed and immutable.** An object is identified by a hash of its content and is never modified.
- **The changeset is the unit of work.** A changeset is an entry in the workspace's changeset log (one log per workspace, ADR 0004).
- **History and blame are derived data.** Path history, blame and ancestry queries are computed on the server from the log and the objects. Derived data is rebuildable and is not a source of truth.
- **Landing is performed on the server**, in this order:
  - rebase the changeset onto the current tip;
  - advance the tip by compare-and-swap, so a landing that lost a race to another landing is not applied;
  - run the post-land checks;
  - revert the landing when a check fails.
- **There is no workspace-wide lock** around landing.
- **There is no push.** A client uploads objects; the server creates the changeset.

### Mapping onto the hexagon

The ports below follow from the decisions, named for the conversation and in the application's terms (`docs/architecture/structure.md`). Whether each is earned by two adapters is settled when it is written.

- **Object store** — put and get immutable objects by content address. Served by an S3-compatible adapter; the same technology also holds filesystem images, which `docs/architecture/execution.md` describes.
- **Changeset log** — append a changeset, read the log, and move the tip by compare-and-swap. Its storage is open.
- **Derived data** — read and write computed history and blame structures for a workspace.
- **Check dispatch** — ask for the post-land checks to run and receive their results. The execution side is in `docs/architecture/runner.md`.
- Driving adapters are the HTTP surface behind `ch` and the MCP server (`docs/architecture/interfaces.md`).

## Rationale

- Server-centred source control, content-addressed objects, changesets and derived data follow the model in Anthropic's account of its own system (https://www.anthropic.com/engineering/redesigning-source-control); the decision and its rejected alternatives are in ADR 0003.
- Tenancy that scopes the log to a workspace is ADR 0004.
- Bundle-based export is ADR 0005.

## Open questions

Each is undecided; the recommendation is not a decision.

- **Metadata database.** Holds workspace, changeset index and tip data, unless the log store holds them.
  - Options: PostgreSQL; SQLite behind a single writer; a distributed SQL database.
  - Recommendation: PostgreSQL for the single-machine initial deployment, behind the port so it can change.
- **Changeset log storage.** Options: a table in the metadata database; an append-only file or segment set in the object store with an index in the database.
  - Recommendation: a table in the metadata database, because compare-and-swap on the tip and the append then share one transaction.
- **What the compare-and-swap compares.** Options: the tip changeset identifier; a per-workspace sequence number.
  - Recommendation: the tip changeset identifier, which needs no second counter.
- **Conflict policy at landing.** What happens when the rebase onto the tip conflicts. Options: reject the landing and return the conflicting paths to the author; hand the conflict to the author's agent to resolve and retry; a server-side resolution policy per path.
  - Recommendation: reject and return the conflict, since it decides nothing about resolution.
- **Which checks run after landing, and who configures them.** Options: per workspace configuration stored as workspace data; a server-wide set.
  - Recommendation: per workspace.
- **When revert is automatic.** Options: any failing check reverts; only checks marked blocking revert; a person or agent decides.
  - Recommendation: only checks marked blocking.
- **Revert of a landing that later changesets depend on.** Options: revert as a new changeset and rebase dependants; refuse the revert while dependants exist.
  - Recommendation: revert as a new changeset.
- **Derived data computation.** Eager, deferred, or by type. Recommendation: deferred for history and blame, with the first query paying for the computation.
- **Retention and deletion of objects** no longer referenced by any changeset. Options: never delete; garbage-collect after a grace period.
  - Recommendation: garbage-collect after a grace period, with the export bundle (`workspace.md`) as the safety copy.
- **Client cache and checkout.** How a client lays out a working copy from cached objects; no option has been examined.
