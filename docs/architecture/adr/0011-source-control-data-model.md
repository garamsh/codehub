# ADR 0011: Objects are SHA-256 addressed, changesets are deltas, and trees and file history are derived

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0003 made source control server-centred with content-addressed objects, changesets and derived data, and left the data model open. The user confirmed the data model in the design session recorded in issue #12 (decisions 1 to 5). Git compatibility is not a goal, since clients do not hold the repository locally (ADR 0003).

The reference model is Anthropic's account of Scion (https://www.anthropic.com/engineering/redesigning-source-control). What this ADR takes from it:

- A Scion changeset stores the list of files that changed relative to its first parent, with content-addressed pointers to the file contents in blob storage.
- Derived data there (file blame, path history, skip lists for the commit graph) is defined recursively: its value for a changeset is computed from its values for that changeset's parents, and each type is computed eagerly or deferred.
- The article names no hash algorithm.

Forces recorded in issue #12:

- Object bytes move between client and object store directly. An S3 upload through a presigned URL signed with AWS Signature Version 4 can carry an `x-amz-checksum-sha256` header (https://docs.aws.amazon.com/AmazonS3/latest/userguide/using-presigned-url.html), and S3 rejects a single-part upload whose computed checksum does not match the supplied one (https://docs.aws.amazon.com/AmazonS3/latest/userguide/checking-object-integrity-upload.html, "If the values don't match, the request is rejected."). With SHA-256 as the address, the object store itself refuses content that does not match its address.
- Checkout and sparse fetch start from a root tree and request missing subtrees and blobs recursively.

The per-file history node takes the shape of a file unode in Meta's Mononoke: a file unode holds the unodes of the same path in the parent changesets, the content identifier, the file type, a path hash and the changeset that introduced it (`eden/mononoke/mononoke_types/src/unode.rs`, `struct FileUnode`, in `facebook/sapling` at commit `d9b33005f8949de7704c1e3f85f3ca506b4b7474`: https://github.com/facebook/sapling/blob/d9b33005f8949de7704c1e3f85f3ca506b4b7474/eden/mononoke/mononoke_types/src/unode.rs).

Alternatives not taken:

- BLAKE3 as the address. It is absent from the checksum algorithms S3 verifies on upload (https://docs.aws.amazon.com/AmazonS3/latest/userguide/checking-object-integrity-upload.html, "Using supported checksum algorithms"), so the store could not reject mismatching content.
- Snapshot changesets, each pointing at a complete tree, as the source of truth.
- Flat manifests listing every path of a version in one object.

Issue #12 states no reason against the last two beyond the decisions themselves.

## Decision

- **Object identity.** Objects are addressed by SHA-256, written with an algorithm prefix (`sha256:<hex>`) so that a later algorithm can coexist.
- **Changeset.** A changeset is a delta against its first parent: the parent, the changed paths each with its content address or a deletion, and metadata. The delta log is the source of truth.
- **Trees.** Merkle directory trees are derived data, computed eagerly at landing. The tree format reserves a place for a chunked large-file entry (content-defined chunking); chunking itself is not implemented.
- **Per-file history.** For each changed file, a file-version node points at the previous version node of that path, as derived data. Blame is derived from these nodes and is deferred. Ancestry queries use skip lists.
- **One derivation mechanism.** Every derived type is computed recursively from its values for the parents, and each type is flagged eager or deferred.

The resulting shape is in `docs/architecture/source-control.md`.

## Consequences

- An object uploaded to S3 in a single part is checked against its address by the store, without the server reading the bytes.
- The address names its algorithm, so a second algorithm can be added without rewriting existing addresses.
- A changeset is small and its size follows the change, not the workspace. Reading a full version needs its tree, so a tree must exist before checkout of that version can start; that is why trees are eager.
- Trees, file-version nodes, blame and skip lists can all be dropped and rebuilt from the log and the blobs; a bug in a derivation is repaired by recomputing, not by migrating the source of truth.
- Landing pays for computing the tree of every landed changeset. Blame is paid for by its first query.
- A derived type whose recursion is wrong for one changeset is wrong for every descendant computed from it.
- Adding a derived type means writing one recursive step and choosing its flag; no second mechanism exists to compute derived data another way.
- Until chunking is implemented, a large file is one object, and a small change to it stores a new copy of the whole file.
- S3 verifies SHA-256 for a multipart upload only as a composite of part checksums, not over the whole object (https://docs.aws.amazon.com/AmazonS3/latest/userguide/checking-object-integrity-upload.html, "Multipart uploads"), so an object uploaded in parts is not verified against its address by the store.
- The serialisation, compression and chunking parameters are left open in `docs/architecture/source-control.md` §Open questions.

## Signs the decision was wrong

- The S3-compatible store chosen for deployment does not verify `x-amz-checksum-sha256` on a presigned upload, so the reason SHA-256 was chosen buys nothing and a faster hash would have cost nothing.
- Most objects exceed the single-request upload size and are uploaded in parts, so the store's whole-object verification applies to few of them.
- SHA-256 hashing shows up as a measured bottleneck on the client or the server for large uploads.
- Computing the tree at landing makes landing latency grow with the depth or width of the changed directories, to the point that landings queue behind it.
- Common queries — reading a version's full file list, diffing two distant versions — repeatedly walk long delta chains or trees where a snapshot or flat manifest would have answered in one read.
- A derived type is found that cannot be computed from its values for the parents alone, such as one needing the whole history at each step, and a second mechanism has to be added beside the recursive one.
- Deferred blame makes first queries slow enough that clients time out, and blame has to be made eager in practice.
- Large binary files arrive before chunking is implemented and the reserved tree entry turns out not to fit content-defined chunking without a format change.
