# Source control

How versions of a workspace's content are stored, changed, landed and queried.

## Current decisions

- **The server holds the truth.** Clients (`ch`, the MCP server, runners) keep at most a bounded cache of objects. Everything that creates a changeset, rebases, lands or reverts runs on the server (ADR 0003).
- **Objects are content-addressed and immutable.** An object is identified by the SHA-256 of its content, written with an algorithm prefix (`sha256:<hex>`) so that a later algorithm can coexist, and is never modified (ADR 0011).
- **The changeset is the unit of work.** A changeset is an entry in the workspace's changeset log (one log per workspace, ADR 0004).
- **A changeset is a delta against its first parent:** the parent, the changed paths each with its content address or a deletion, and metadata. The delta log is the source of truth (ADR 0011).
- **Trees, history and blame are derived data.** Trees, path history, blame and ancestry queries are computed on the server from the log and the objects. Derived data is rebuildable and is not a source of truth.
- **One derivation mechanism.** Every derived type is computed recursively from its values for the parents, and each type is flagged eager or deferred (ADR 0011). The types and their flags:
  - **Merkle directory trees** — eager, computed at landing, because checkout and sparse fetch start from a root tree and request missing subtrees and blobs recursively. The tree format reserves a place for a chunked large-file entry (content-defined chunking); chunking is not implemented.
  - **File-version nodes** — one per changed file, pointing at the previous version node of the same path.
  - **Blame** — deferred, derived from the file-version nodes.
  - **Skip lists** — over the changeset graph, answering ancestry queries.
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
- **Derived data** — read and write computed trees, file-version nodes, blame and skip lists for a workspace.
- **Check dispatch** — ask for the post-land checks to run and receive their results. The execution side is in `docs/architecture/runner.md`.
- Driving adapters are the server API behind `ch` and the MCP server (`docs/architecture/interfaces.md`).

## Rationale

- Server-centred source control, content-addressed objects, changesets and derived data follow the model in Anthropic's account of its own system (https://www.anthropic.com/engineering/redesigning-source-control); the decision and its rejected alternatives are in ADR 0003.
- SHA-256 addressing, delta changesets, derived trees and file-version nodes, and the single derivation mechanism are ADR 0011: SHA-256 because an S3 presigned upload can carry its checksum and the store then rejects content that does not match its address; trees eager because checkout starts from them. BLAKE3, snapshot changesets and flat manifests were not taken.
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
- **Flags of file-version nodes and skip lists.** ADR 0011 makes the flag per type and fixes trees as eager and blame as deferred; it does not flag these two. Options for each: eager at landing; deferred to the first query.
  - Recommendation: deferred, with the first query paying for the computation.
- **Serialisation of trees, changesets and file-version nodes.** The byte layout each is hashed and stored in, including the reserved chunked-file entry of a tree. No option has been examined.
- **Changeset identifier.** Options: the SHA-256 address of the serialised changeset; an identifier assigned by the log.
- **Compression of stored objects**, and whether the address covers the stored or the original bytes. The object store verifies the checksum of the bytes it receives, so compressing before upload with the address over the original bytes forgoes that check. No option has been examined.
- **Objects uploaded in parts.** S3 verifies SHA-256 for a multipart upload only as a composite of part checksums (https://docs.aws.amazon.com/AmazonS3/latest/userguide/checking-object-integrity-upload.html, "Multipart uploads"), so such an object is not checked against its address by the store. Options: a size limit that keeps every object a single-part upload; server-side verification after a multipart upload.
- **Chunking parameters.** The content-defined chunking algorithm, its target, minimum and maximum chunk sizes, and the file size above which a file is chunked. No option has been examined.
- **Skip-list spacing.** Which ancestors each changeset's skip list points at. No option has been examined.
- **Retention and deletion of objects** no longer referenced by any changeset. Options: never delete; garbage-collect after a grace period.
  - Recommendation: garbage-collect after a grace period, with the export bundle (`workspace.md`) as the safety copy.
- **Client cache and checkout.** How a client lays out a working copy from cached objects; no option has been examined.
