# ADR 0007: Caches are versioned workspace data with images derived from them

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

Jobs need dependency and build caches that are large, made of many small files, and shared by hundreds of concurrent agents. The design session recorded in issue #1 (item 4) settled how caches relate to the workspace. Three models were weighed: sharing files into the execution environment, exposing disks to users as a concept, and treating caches as workspace data from which mountable images are derived (model C).

## Decision

Cache model C. Users see only the workspace. A cache is versioned workspace data, and workspace objects are the source of truth. A read-only filesystem image used for mounting is derived data: rebuildable, cacheable anywhere, and excluded from export. A job sees its layers stacked: the organisation base, the workspace cache, and the source at a changeset, plus a per-job writable scratch layer. Promoting a new cache version is a server policy limited to trusted jobs.

## Consequences

- There is one storage system, the workspace's objects; export has nothing extra to carry.
- Any image can be discarded and rebuilt from objects, so image storage and host caches need no durability.
- The definition of a trusted job and the promotion policy become server code with a security role.
- Building an image from objects is a cost the system pays on each new cache version.
- File-level sharing into the environment is rejected: it is slow for many small files, and virtiofs is not available in every VMM. Disks as a user-facing concept are rejected: they add a second storage system and make export harder.

## Signs the decision was wrong

- Rebuilding images from objects is slower or costlier than the jobs it speeds up.
- A user needs to address a cache or image directly, which means the workspace-only view does not suffice.
- Cache poisoning occurs through a path the trusted-job policy does not cover.
- Images turn out to hold state that cannot be rebuilt from objects.
