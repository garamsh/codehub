# Execution

How a job's environment is built from workspace data: caches, images, their delivery to a runner, and the executor drivers that run the job.

## Current decisions

### Caches and images (ADR 0007)

- Users see only the workspace. A cache is versioned workspace data; workspace objects are the source of truth.
- A read-only filesystem image used for mounting is derived data: rebuildable, cacheable anywhere, excluded from export.
- A job's filesystem is layers, bottom to top: the organisation base, the workspace cache, the source at a changeset, and a per-job writable scratch layer. The scratch layer is discarded with the job's environment.
- Promoting a new cache version is a server policy limited to trusted jobs, because an untrusted job writing the cache would poison later jobs.

### Delivery (ADR 0008)

- Images live in object storage behind an S3-compatible API.
- A runner fetches an image through a short-lived presigned URL the server issues, caches it on its host keyed by version hash, and refetches only when the version changes.
- Images are kept as image files, not extracted.

### Executors (ADR 0008)

- The server and the runner core do not know the execution environment.
- An executor driver both runs the job and attaches the images. Drivers: microvm (default), kubernetes (images attached via CSI), host.
- The VMM is not chosen.

### Ports

Declared in the application's terms (`docs/architecture/structure.md`); the adapters are the technologies named here.

- **Image store** — stores and retrieves derived images and issues time-limited access to them. Adapter: S3-compatible object storage, with presigned URLs.
- **Image builder** — derives an image from workspace objects for a cache version. Adapter: not decided, see open questions.
- **Executor driver** — runs one job in a disposable environment with the given images attached. Adapters: microvm, kubernetes, host. Lives on the runner side of the hexagon; the runner core calls it without knowing which it is.
- **Workspace objects** — the source of truth images are derived from. Described in `source-control.md` and `workspace.md`, not here.

## Rationale

- Cache model C and the rejected alternatives: ADR 0007.
- Object-storage delivery and executor drivers: ADR 0008.
- Export excludes images, so export content is defined by `workspace.md`.

## Open questions

- **VMM for the microvm driver.** Options: Firecracker, Cloud Hypervisor, QEMU microvm. Recommendation: choose after the image attach mechanism is fixed, because it decides which VMMs qualify.
- **Image filesystem format.** Options: EROFS, SquashFS, ext4. Recommendation: EROFS for read-only layers, if the driver kernels support it; to be confirmed by measurement.
- **Layer stacking mechanism.** Options: overlay of read-only images with a writable layer, or one merged image per job. Recommendation: overlay, since layers change at different rates.
- **Chunking and lazy fetch.** Options: whole-file fetch, chunked images fetched on demand. Recommendation: whole-file first, chunking when image size makes fetch time dominate.
- **Cache eviction.** Options: least recently used per host, a fixed per-host budget, explicit retention per workspace. Recommendation: least recently used within a per-host budget.
- **Scheduling by cache locality.** Options: ignore, prefer runners already holding the version, require it. Recommendation: prefer; expressed through the label and pool routing in `runner.md`.
- **Definition of a trusted job.** Options: jobs on landed changesets only, jobs on named branches of changesets, an explicit workspace setting. Recommendation: jobs on landed changesets; depends on `access.md`.
