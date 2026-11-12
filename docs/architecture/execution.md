# Execution

How a job's environment is built from repository data: caches, images, their delivery to a runner, and the executor drivers that run the job.

## Current decisions

### Caches and images (ADR 0007)

- Users see only the repository. A cache is versioned repository data; repository objects are the source of truth.
- A read-only filesystem image used for mounting is derived data: rebuildable, cacheable anywhere, excluded from export.
- An organisation holds a base layer shared by its repositories (`tenancy.md`). How it and other layers combine in a job is open.
- Promoting a new cache version is a server policy limited to trusted jobs, because an untrusted job writing the cache would poison later jobs.

### Delivery (ADR 0008)

- Images live in object storage behind an S3-compatible API.
- A runner fetches an image through a short-lived presigned URL the server issues, caches it on its host keyed by version hash, and refetches only when the version changes.
- Images are kept as image files, not extracted.

### Executors (ADR 0008)

- The server and the runner core do not know the execution environment.
- An executor driver both runs the job and attaches the images. Drivers: microvm (default), kubernetes (images attached via CSI), host.
- **Only the host driver is built** (ADR 0034). It runs a job's command under `/bin/sh -c` in a fresh directory on the runner's host, fetches the changeset's files by presigned URL and checks each against its address, stops the job's process group on cancel or timeout, and removes the directory. It isolates nothing: a runner on it must be trusted with what its jobs run. No image is attached.
- The VMM is not chosen.

### Ports

Declared in the application's terms (`docs/architecture/structure.md`); the adapters are the technologies named here.

- **Image store** — stores and retrieves derived images and issues time-limited access to them. Adapter: S3-compatible object storage, with presigned URLs.
- **Image builder** — derives an image from repository objects for a cache version. Adapter: not decided, see open questions.
- **Executor driver** — runs one job in a disposable environment with the given images attached. Adapters: microvm, kubernetes, host. Lives on the runner side of the hexagon; the runner core calls it without knowing which it is.
- **Repository objects** — the source of truth images are derived from. Described in `source-control.md` and `tenancy.md`, not here.

## Rationale

- Cache model C and the rejected alternatives: ADR 0007.
- Object-storage delivery and executor drivers: ADR 0008.
- The host driver as the only driver built so far: ADR 0034.
- Export excludes images, so export content is defined by `tenancy.md`.

## Open questions

- **VMM for the microvm driver.** Options: Firecracker, Cloud Hypervisor, QEMU microvm. Recommendation: choose after the image attach mechanism is fixed, because it decides which VMMs qualify.
- **Image filesystem format.** Options: EROFS, SquashFS, ext4. Recommendation: EROFS for read-only layers, if the driver kernels support it; to be confirmed by measurement.
- **Layer composition in a job.** Which layers a job sees (organisation base, repository cache, source at a changeset, a per-job writable scratch layer) and how they combine. Options: overlay of read-only images with a writable scratch layer; one merged image per job. Recommendation: the four layers above, combined by overlay, since they change at different rates; the scratch layer is discarded with the job's environment.
- **Chunking and lazy fetch.** Options: whole-file fetch, chunked images fetched on demand. Recommendation: whole-file first, chunking when image size makes fetch time dominate.
- **Cache eviction.** Options: least recently used per host, a fixed per-host budget, explicit retention per repository. Recommendation: least recently used within a per-host budget.
- **Scheduling by cache locality.** Options: ignore, prefer runners already holding the version, require it. Recommendation: prefer; expressed through the label and pool routing in `runner.md`.
- **Definition of a trusted job.** Options: jobs on landed changesets only, jobs on named branches of changesets, an explicit repository setting. Recommendation: jobs on landed changesets; depends on `access.md`.
