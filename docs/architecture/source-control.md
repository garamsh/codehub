# Source control

How versions of a repository's content are stored, changed, landed and queried.

## Current decisions

- **The server holds the truth.** Clients (`ch`, runners) keep at most a bounded cache of objects. Everything that creates a changeset, rebases, lands or reverts runs on the server (ADR 0003).
- **Objects are content-addressed and immutable.** An object is identified by the SHA-256 of its content, written with an algorithm prefix (`sha256:<hex>`) so that a later algorithm can coexist, and is never modified (ADR 0011).
- **Changesets, trees and file-version nodes have exactly one encoding each**, specified in §Object encoding, and an object's address is the SHA-256 of its encoded bytes (ADR 0016).
- **A changeset is identified by its address**, the SHA-256 of its encoding, and not by anything the log assigns (ADR 0016).
- **The changeset is the unit of work.** A changeset is an entry in the repository's changeset log (one log per repository, ADR 0004).
- **A changeset is a delta against its first parent:** the parent, the changed paths each with its content address or a deletion, and metadata. The delta log is the source of truth (ADR 0011).
- **Trees, history and blame are derived data.** Trees, path history, blame and ancestry queries are computed on the server from the log and the objects. Derived data is rebuildable and is not a source of truth.
- **One derivation mechanism.** Every derived type is computed recursively from its values for the parents, and each type is flagged eager or deferred (ADR 0011). The types and their flags:
  - **Merkle directory trees** — eager, computed at landing, because checkout and sparse fetch start from a root tree and request missing subtrees and blobs recursively. The tree format reserves a place for a chunked large-file entry (content-defined chunking); chunking is not implemented.
  - **File-version nodes** — deferred, one per changed file, pointing at the previous version node of the same path.
  - **Blame** — deferred, derived from the file-version nodes; not implemented.
  - **Skip lists** — over the changeset graph, answering ancestry queries; not implemented.
- **A path's history follows each node's first previous node:** from the path's file-version node in a changeset back through the first entry of each node's previous list, newest first. The other entries of a merge's previous list are kept and not walked.
- **A repository has named refs** (ADR 0012):
  - `main` — what agents base their work on, and what landing advances;
  - release refs;
  - `last-green` — what CI and deployment base on.
  - `main` and `last-green` are implemented; release refs are not.
- **PostgreSQL holds the directory, the changeset log, refs, drafts, derived data and grants** in one schema shared by all organisations, with a repository identifier on every row below the directory. The minimum major version is 18, the server takes a connection URL, migrations are forward-only SQL files in `crates/adapter-postgres` applied at server start, and connections are pooled in process, 40 per server process by default, with the sum over processes kept below PostgreSQL's `max_connections` (ADR 0022, ADR 0037).
- **A draft is identified by repository, principal and draft name** (ADR 0022).
- **A landing that lost the compare-and-swap leaves its changeset and derived rows**, which a retry reuses because they are content-addressed; removal waits for the retention decision (ADR 0022).
- **Object keys are `<repository id>/<sha256 hex>`.** Uploads are single-part with `x-amz-checksum-sha256` signed, so the store verifies the bytes against the address; objects are not compressed, so the address covers the stored bytes. The server refuses a grant above a configured maximum no larger than S3's single-PUT limit, and presigned URLs expire after a configured time, 15 minutes by default (ADR 0023).
- **There are no work branches.** Pending work is a server-side draft changeset outside the log (ADR 0012).
- **A draft is the caller's workspace** (ADR 0048):
  - its base is fixed at `main` when it is opened, and is absent where `main` is unset, and it carries a description that a landing without a message takes as its message;
  - it closes when it lands, and one workspace serves one task, so following `main` within a task is not provided;
  - its changes are recorded path by path or as one snapshot of several paths applied together, and a restore removes a path's change;
  - the server answers what it changes against its base, as statuses and as a line diff.
- **A changeset's files are listed with a download grant each**, under an optional path prefix; read is asked for every listed path in one decision, and a caller holding no `read` grant for any one of them is refused the whole listing (ADR 0048).
- **Moving a ref is a compare-and-swap on that ref alone**, comparing the ref's current changeset identifier, and is separate from creating the changeset (ADR 0012).
- **Landing is performed on the server**, in this order:
  - rebase the changeset onto the current `main`;
  - advance `main` by compare-and-swap, so a landing that lost a race to another landing is not applied;
  - on a lost swap, read `main` again, rebase the same delta and swap again, up to a bound set by the server (32 by default), with authorisation and the draft read done once; when the bound is spent the landing is answered as moved (ADR 0036);
  - queue the post-land checks;
  - revert the landing when a blocking check fails.
- **Post-land checks are configured in the repository** (ADR 0035). `.codehub/checks.toml` in the landed tree lists checks, each with a name, a command, labels, `blocking` (default true) and a timeout (default 600 s). Each landing on `main` queues one job per check (`runner.md`). A file that does not parse counts as one failed blocking check.
- **`last-green` moves forward only**, by compare-and-swap, to a changeset on `main` whose blocking checks have all passed; a changeset with no blocking checks passes on landing (ADR 0035).
- **A blocking check that fails or times out reverts its changeset** as a new changeset on `main`, made by the landing merge, authored by the server and naming the changeset and the check. A changeset is reverted at most once, an automatic revert is not itself reverted, a revert that conflicts lands nothing and is recorded with the check result, and a revert that fails after it was reserved lands nothing and is recorded with the check result and the reason, and is not tried again. Cancelled and non-blocking checks revert nothing (ADR 0035).
- **The rebase at landing merges as follows** (ADR 0012):
  - a changeset whose paths do not overlap the changes made since its base is re-parented onto `main` with no merge;
  - overlapping text files are merged by a line-level 3-way merge;
  - overlapping binary files are merged at path level;
  - a true conflict rejects the landing and returns the conflicting paths for the author's agent to resolve.
- **Details of the landing merge** (ADR 0017):
  - lines are diffed with the Histogram algorithm of the `imara-diff` crate; the 3-way merge over those diffs is the domain ring's own;
  - a file version is binary when a NUL byte occurs in its first 8000 bytes or it is not valid UTF-8, and text otherwise; a path is merged by lines only when its base and both sides are text;
  - lines are compared byte for byte, so whitespace-only and line-ending differences are differences;
  - renames are not detected: a rename is a deletion and an addition;
  - true conflicts are a deletion against a modification, an addition on both sides with different content, both sides changing overlapping base line ranges or inserting at the same base position differently, and both sides changing a binary path to different content;
  - identical changes on both sides, a deletion on both sides, and changes to disjoint line ranges of one text file, adjacent ones included, are clean.
- **The file kind and a file against a directory** (ADR 0019):
  - a path's kind, file or executable file, is merged 3-way apart from its content: a kind changed on one side is taken, and both sides setting it to different kinds is a conflict; where the content conflicts too, the content conflict is reported;
  - a delta that leaves a path holding a file while another path lies under it is rejected where the delta is applied to a tree, and the landing reports it as a conflict on the path holding the file; the per-path merge does not detect it.
- **Landings of one repository pass a gate in each server process**, one at a time and in arrival order, after authorisation and the draft read; the gate is held for the landing's attempts at the swap. There is no lock across processes, and an automatic revert does not pass the gate (ADR 0039).
- **There is no push.** A client uploads objects; the server creates the changeset.
- **PostgreSQL holds the metadata, the changeset log as a table, and the refs**, each behind a port (ADR 0013).
- **Blobs are in the S3-compatible object store** (ADR 0013).
- **The ports leave room for a distributed SQL store later:** they name no PostgreSQL feature (ADR 0013).
- **The move to a distributed SQL store is proposed only when the PostgreSQL primary is measured as the bottleneck**, after the largest machine, an external pooler and log partitioning, and reproduced with `crates/loadtest` (ADR 0040).

### Object encoding

Format version 1 of the encoding of ADR 0016. A value has exactly one encoding; a decoder rejects input that is not it.

Field types:

| Type | Bytes |
|---|---|
| `u8` | one unsigned byte |
| `u32` | four bytes, unsigned, big-endian |
| `i64` | eight bytes, two's complement, big-endian |
| `bytes` | a `u32` length, then that many bytes |
| `text` | `bytes` holding valid UTF-8 |
| `list<T>` | a `u32` count, then that many `T` |
| `address` | a `u8` algorithm (`0x01` = SHA-256), then the digest as `bytes` (32 for SHA-256) |
| `component` | `bytes`, non-empty, holding no `/` (`0x2F`) and no NUL (`0x00`), and neither `.` nor `..` |
| `path` | a `list<component>` with at least one component, from the repository root down |

Ordering: components compare by unsigned bytes, a proper prefix first; paths compare component by component, a proper prefix first.

Every encoded object opens with a header of two `u8`: the format version (`0x01`), then the object kind (`0x01` changeset, `0x02` tree, `0x03` file-version node).

Entry kinds, one `u8`:

| Code | Kind | Followed by | Allowed in |
|---|---|---|---|
| `0x00` | deletion | nothing | changeset change |
| `0x01` | file | the blob's `address` | changeset change, tree entry, file-version node |
| `0x02` | executable file | the blob's `address` | changeset change, tree entry, file-version node |
| `0x03` | directory | the subtree's `address` | tree entry |
| `0x04` | chunked file | reserved; rejected until chunking is specified | none in format version 1 |

The executable bit is the only file mode. Symbolic links are not represented in format version 1.

Changeset, fields in order:

1. Header, kind `0x01`.
2. Parents: `list<address>` of changeset identifiers. The first is the parent the delta is against; the list is empty for a repository's first changeset.
3. Author: `text`.
4. Timestamp: `i64`, seconds since 1970-01-01T00:00:00Z.
5. Message: `text`.
6. Changes: `list` of change, sorted by path, no path twice. A change is a `path`, an entry kind (`0x00`, `0x01` or `0x02`), and what that kind is followed by.

Tree, fields in order:

1. Header, kind `0x02`.
2. Entries: `list` of entry, sorted by name, no name twice. An entry is a name (`component`), an entry kind (`0x01`, `0x02` or `0x03`), and what that kind is followed by.

Only a root tree has no entries; a directory left with none is removed from its parent.

File-version node, fields in order:

1. Header, kind `0x03`.
2. Changeset: the `address` of the changeset that introduced this version.
3. Path: `path`.
4. Content: an entry kind (`0x01` or `0x02`) and what that kind is followed by.
5. Previous: `list<address>` of the file-version nodes of the same path in the parent changesets, in parent order; empty where no parent has the path.

A deletion creates no file-version node.

Addresses:

- An encoded object's address is the SHA-256 of all its encoded bytes, header included.
- A blob's address is the SHA-256 of the file content as given; a blob has no header.
- A changeset's identifier is its address.

### Mapping onto the hexagon

The ports below follow from the decisions, named for the conversation and in the application's terms (`docs/architecture/structure.md`). Whether each is earned by two adapters is settled when it is written.

- **Object store** — put and get immutable objects by content address, answer an object's size without its bytes, and issue presigned URLs through which clients upload and download object bytes directly to and from the store, so the bytes do not pass through the server API (ADR 0015). Served by an S3-compatible adapter; the same technology also holds filesystem images, delivered the same way (`docs/architecture/execution.md`, ADR 0008).
- **Changeset log** — append a changeset, read the log, read a ref, and move a ref by compare-and-swap on that ref alone. Served by a PostgreSQL adapter, the log as a table (ADR 0013, ADR 0022).
- **Derived data** — read and write computed trees, file-version nodes, blame and skip lists for a repository. Served by PostgreSQL (ADR 0022).
- **Draft store** — put and get draft changesets with their description, held outside the log until they land (ADR 0012, ADR 0048). Served by PostgreSQL (ADR 0022).
- **Check dispatch** — ask for the post-land checks to run and receive their results. Served by the job queue port (`docs/architecture/runner.md`).
- **Check definitions** — read the checks a changeset declares from the bytes of `.codehub/checks.toml`. Served by a TOML adapter, and an in-memory adapter for tests (ADR 0035).
- Driving adapters are the server API behind `ch` (`docs/architecture/interfaces.md`). They reach the application through four driving ports: **source control** (open, read, describe, snapshot, restore and diff a workspace, stage a change into a draft, grant an upload, land a draft on `main`, read a file, list a changeset's files and read a path's history), **repositories** (create organisations and repositories), **members** (add and remove members and grants) and **authentication** (verify the credential presented with a request).

## Rationale

- Server-centred source control, content-addressed objects, changesets and derived data follow the model in Anthropic's account of its own system (https://www.anthropic.com/engineering/redesigning-source-control); the decision and its rejected alternatives are in ADR 0003.
- SHA-256 addressing, delta changesets, derived trees and file-version nodes, and the single derivation mechanism are ADR 0011: SHA-256 because an S3 presigned upload can carry its checksum and the store then rejects content that does not match its address; trees eager because checkout starts from them. BLAKE3, snapshot changesets and flat manifests were not taken.
- Named refs, the absence of work branches and the landing merge are ADR 0012: line-level merge because rejecting non-overlapping edits to one file would cause retry storms among hundreds of agents; re-parenting because it keeps landing cost proportional to the change; `last-green` because a broken `main` contaminates work based on it until the revert. Work branches, path-level merge of text, a merge changeset for every landing, CI basing on `main` and server-side conflict resolution were not taken.
- The merge details are ADR 0017: Histogram because, as a patience variant, it misaligns lines less often than Myers, which in a 3-way merge means fewer spurious conflicts; text by content because it needs no configuration; no rename detection because a wrong pairing merges edits into the wrong file. Myers diff, rename detection in M1, whitespace-insensitive merge and a per-path text attribute were not taken.
- The file kind and a file against a directory are ADR 0019: the kind apart from content, so a kind change on one side and a content change on the other both survive; a file against a directory at tree application, because the two paths never meet in the per-path merge and the tree cannot hold both. The kind as part of the content, one side's kind always winning, a per-path rule for a file against a directory and resolving it by preferring one side were not taken.
- The project-defined encoding and the changeset identifier are ADR 0016: one encoding per value, because an address is a hash of bytes; the identifier as the address, so a changeset is checked like any other object and is named before it is appended. Deterministic CBOR and an identifier assigned by the log were not taken.
- PostgreSQL for the metadata, the log and the refs, and the object store for blobs, are ADR 0013. SQLite behind a single writer, a distributed SQL database now, and log segments in the object store were not taken.
- Why a draft is a workspace snapshotted by `ch` rather than staged file by file, and why the diff is the server's: ADR 0048. A lazy filesystem view, a syncing daemon, landing part of a workspace and a client-side diff were not taken.
- Where derived data and drafts live, how PostgreSQL is run, and what a lost landing leaves: ADR 0022. A shared schema was taken over a schema per organisation, and the object store for derived data was not taken.
- The retry of a lost swap inside the server, measured against the client's retry: ADR 0036. The default pool size and the bound on the sum of pools: ADR 0037. The fair gate per process, over a lock across replicas and over a limit on waiting landings: ADR 0039. All three rest on `docs/scale-baseline.md`.
- The trigger for distributed SQL, over moving now, a trigger by agent count and leaving it open: ADR 0040.
- The object key, checksum verification, no compression and single-part uploads are ADR 0023: per-repository keys because export and deletion are per repository; compression and multipart upload not taken because each forgoes the store's check of the bytes against the address. Facts checked on 2026-11-11 are cited there.
- Checks in the repository, `last-green` on blocking checks only, and automatic revert as a new changeset: ADR 0035, with the alternatives not taken.
- Tenancy that scopes the log to a repository is ADR 0004.
- Bundle-based export is ADR 0005.

## Open questions

Each is undecided; the recommendation is not a decision.

- **How release refs are created and moved, and by whom.** No option has been examined.
- **Draft changeset lifetime.** How long a draft outside the log is kept and what removes it. No option has been examined. A draft is keyed by its principal, so no caller can name another principal's draft.
- **Landings whose checks were never queued**, because the server stopped between the compare-and-swap and queuing (ADR 0035). Options: accept; find them at server start by comparing `main` after `last-green` with the queued jobs. Recommendation: decide when one is observed.
- **Flag of skip lists.** ADR 0011 makes the flag per type and fixes trees as eager and blame as deferred; it does not flag skip lists. Options: eager at landing; deferred to the first query.
  - Recommendation: deferred, with the first query paying for the computation.
- **History through a merge.** Whether a path's history walks the versions a merge's other parents reached it by, and in what order. Options: only the first previous node, as now; every previous node, in an order still to be chosen.
- **Chunking parameters.** The content-defined chunking algorithm, its target, minimum and maximum chunk sizes, and the file size above which a file is chunked. No option has been examined.
- **Skip-list spacing.** Which ancestors each changeset's skip list points at. No option has been examined.
- **Retention and deletion of objects** no longer referenced by any changeset. Options: never delete; garbage-collect after a grace period.
  - Recommendation: garbage-collect after a grace period, with the export bundle (`tenancy.md`) as the safety copy.
- **Client cache and checkout.** How a client lays out a working copy from cached objects; no option has been examined.
