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
- **A workspace has named refs** (ADR 0012):
  - `main` — what agents base their work on, and what landing advances;
  - release refs;
  - `last-green` — what CI and deployment base on.
  - Only `main` is implemented first; the format admits more refs.
- **There are no work branches.** Pending work is a server-side draft changeset outside the log (ADR 0012).
- **Moving a ref is a compare-and-swap on that ref alone**, comparing the ref's current changeset identifier, and is separate from creating the changeset (ADR 0012).
- **Landing is performed on the server**, in this order:
  - rebase the changeset onto the current `main`;
  - advance `main` by compare-and-swap, so a landing that lost a race to another landing is not applied;
  - run the post-land checks;
  - revert the landing when a check fails.
- **The rebase at landing merges as follows** (ADR 0012):
  - a changeset whose paths do not overlap the changes made since its base is re-parented onto `main` with no merge;
  - overlapping text files are merged by a line-level 3-way merge;
  - overlapping binary files are merged at path level;
  - a true conflict rejects the landing and returns the conflicting paths for the author's agent to resolve.
- **There is no workspace-wide lock** around landing.
- **There is no push.** A client uploads objects; the server creates the changeset.
- **PostgreSQL holds the metadata, the changeset log as a table, and the refs**, each behind a port (ADR 0013).
- **Blobs are in the S3-compatible object store** (ADR 0013).
- **The ports leave room for a distributed SQL store later:** they name no PostgreSQL feature (ADR 0013).

### Mapping onto the hexagon

The ports below follow from the decisions, named for the conversation and in the application's terms (`docs/architecture/structure.md`). Whether each is earned by two adapters is settled when it is written.

- **Object store** — put and get immutable objects by content address, and issue presigned URLs through which clients upload and download object bytes directly to and from the store, so the bytes do not pass through the server API (ADR 0015). Served by an S3-compatible adapter; the same technology also holds filesystem images, delivered the same way (`docs/architecture/execution.md`, ADR 0008).
- **Changeset log** — append a changeset, read the log, read a ref, and move a ref by compare-and-swap on that ref alone. Served by a PostgreSQL adapter, the log as a table (ADR 0013).
- **Derived data** — read and write computed trees, file-version nodes, blame and skip lists for a workspace.
- **Check dispatch** — ask for the post-land checks to run and receive their results. The execution side is in `docs/architecture/runner.md`.
- Driving adapters are the server API behind `ch` and the MCP server (`docs/architecture/interfaces.md`).

## Rationale

- Server-centred source control, content-addressed objects, changesets and derived data follow the model in Anthropic's account of its own system (https://www.anthropic.com/engineering/redesigning-source-control); the decision and its rejected alternatives are in ADR 0003.
- SHA-256 addressing, delta changesets, derived trees and file-version nodes, and the single derivation mechanism are ADR 0011: SHA-256 because an S3 presigned upload can carry its checksum and the store then rejects content that does not match its address; trees eager because checkout starts from them. BLAKE3, snapshot changesets and flat manifests were not taken.
- Named refs, the absence of work branches and the landing merge are ADR 0012: line-level merge because rejecting non-overlapping edits to one file would cause retry storms among hundreds of agents; re-parenting because it keeps landing cost proportional to the change; `last-green` because a broken `main` contaminates work based on it until the revert. Work branches, path-level merge of text, a merge changeset for every landing, CI basing on `main` and server-side conflict resolution were not taken.
- PostgreSQL for the metadata, the log and the refs, and the object store for blobs, are ADR 0013. SQLite behind a single writer, a distributed SQL database now, and log segments in the object store were not taken.
- Tenancy that scopes the log to a workspace is ADR 0004.
- Bundle-based export is ADR 0005.

## Open questions

Each is undecided; the recommendation is not a decision.

- **Where derived data is stored.** ADR 0013 places the metadata, the log and the refs, and not derived data. Options: PostgreSQL; the object store, as content-addressed objects; split by type, such as trees in the object store and skip lists in PostgreSQL.
- **Where draft changesets are stored.** They are outside the log (ADR 0012); no option has been examined.
- **PostgreSQL version and hosting.** Which major version is the minimum, and whether it is run by the project or consumed as a managed service. No option has been examined.
- **Schema migration tooling.** How the schema is versioned and migrated. No option has been examined.
- **Connection pooling.** Whether the server pools connections itself or sits behind an external pooler. No option has been examined.
- **When the move to distributed SQL is made.** What measurement triggers it, such as write latency on the log or on `main` at a given number of concurrent agents. No option has been examined.
- **What moves `last-green`, and on which check results.** Options: the server, when every post-land check of a changeset on `main` passes; the server, when only the checks marked blocking pass; a CI job holding a credential to move it.
- **How release refs are created and moved, and by whom.** No option has been examined.
- **Draft changeset lifetime.** How long a draft outside the log is kept, what removes it, and whether one agent can read another's draft. No option has been examined.
- **Rename and move detection in the landing merge.** Whether a path renamed on one side and edited on the other is merged or is a conflict. No option has been examined.
- **Line-merge algorithm.** Which 3-way line merge, and whether whitespace-only differences count. No option has been examined.
- **Text or binary.** How the merge decides a file is text: by content, by a per-path attribute, or both. No option has been examined.
- **Which overlaps are true conflicts beyond overlapping lines**, such as a deletion on one side against an edit on the other, or the same path added on both sides with different content. No option has been examined.
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
