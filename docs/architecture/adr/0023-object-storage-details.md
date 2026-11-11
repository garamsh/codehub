# ADR 0023: Objects are keyed per repository, verified by checksum, and uploaded in one part

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0011 chose SHA-256 addresses because an S3 presigned upload can carry the checksum, and ADR 0015 moves object bytes by presigned URL. Left open in `docs/architecture/source-control.md` were compression and objects uploaded in parts; the key layout was not recorded.

Alternatives not taken:

- A key of the address alone, shared across repositories. It deduplicates across repositories, but export and deletion are per repository (ADR 0005) and would need a reference count.
- Compressing before upload with the address over the original bytes. The store then cannot verify the bytes against the address.
- Multipart upload. S3 verifies SHA-256 for a multipart upload only as a composite of part checksums (https://docs.aws.amazon.com/AmazonS3/latest/userguide/checking-object-integrity-upload.html, "Multipart uploads"), so the store would not check the object against its address.
- Server-side verification after a multipart upload. It puts the bytes through the server, which ADR 0015 avoids.

Fact checked on 2026-11-11 against https://docs.aws.amazon.com/AmazonS3/latest/userguide/upload-objects.html: with a single PUT operation a single object up to 5 GB can be uploaded.

## Decision

- The object key is `<repository id>/<sha256 hex>`.
- A presigned upload signs `x-amz-checksum-sha256`, so the store verifies the bytes against the address. Objects are not compressed, and the address covers the stored bytes.
- Uploads are single-part only. The server refuses a grant above a configured maximum that is no larger than S3's single-PUT limit of 5 GB.
- Presigned URLs expire after a configured time, 15 minutes by default.

## Consequences

- Export and deletion of a repository's objects are a key-prefix operation.
- The same content in two repositories is stored twice.
- The store rejects an upload whose bytes do not match the address, so a wrong object cannot be stored under an address.
- A file above the configured maximum cannot be stored until chunking (open in `docs/architecture/source-control.md`) or a later decision changes that.
- A client that needs longer than the expiry to upload asks for a new URL.

## Signs the decision was wrong

- Files above the maximum are committed often enough that rejecting them blocks work.
- Storage cost from cross-repository duplicates becomes material.
- Uploads fail on expiry for ordinary file sizes on ordinary connections.
- An S3-compatible store the project targets does not honour `x-amz-checksum-sha256`.
