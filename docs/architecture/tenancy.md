# Tenancy

How the system is divided into organisations and repositories, and how a repository is exported, backed up and imported.

## Current decisions

### Tenancy

- **An organisation contains repositories** (ADR 0004; the term "repository" is ADR 0020, which replaces "workspace" in ADR 0004 and ADR 0005).
- **One repository is one codebase**, with one changeset log (`docs/architecture/source-control.md`).
- **An organisation holds a base layer** that its repositories share.
- **Target scale** is hundreds of concurrent agents.
- **A repository sets no limit on concurrent landings.** Each server process passes the landings of one repository one at a time, in arrival order, and does not bound how many wait (ADR 0039, `source-control.md`).
- **Tenancy scopes every port that reaches stored data**: a request names its repository and, through it, its organisation.
- **Organisation roles and path grants** are decided in ADR 0021 and described in `docs/architecture/access.md`. codehub owns organisations and repositories as tenancy containers; codehub depends on the standards and on no outside service's model (ADR 0028), except garam's names and entity feed in the AuthZEN `grn` profile (ADR 0043, ADR 0044).
- **Organisation names are shared with an outside layer with no mapping table** (ADR 0028). The name is the key on both sides, so an organisation is not renamed on one side alone. A repository is named by its name, except in the `grn` profile, which names it by its id (ADR 0043).
- **In external authorisation mode an organisation is created on first use** when the decision point allows `CreateOrganisation`, and is not created when it denies; no admin grant is written (ADR 0030). In every mode, repository creation is written to an outbox in the repository's transaction and delivered at least once through the directory-change port; creating a repository waits a bounded time for delivery and answers 201 either way (ADR 0044). Organisation creation is reported at most once to the in-process adapter (ADR 0030). Nothing emits deletion until deletion exists (`docs/architecture/access.md`).
- **A repository has an id that is never reused**, the identity column of its row. The GRN profile names a repository by it (ADR 0043).
- **The directory and the metadata are in one shared PostgreSQL schema** with a repository identifier on every row below the directory (ADR 0022).
- Authentication and authorisation of people, agents and runners are outside this document (`docs/architecture/access.md`).

### Export and backup

- **Export, backup and import are per repository** (ADR 0005).
- **The unit is a bundle** of the changeset log, the objects it references, and metadata.
- **Backup is incremental.**
- **Import reads the same bundle** that export and backup write.
- **Derived data is rebuildable** (`docs/architecture/source-control.md`), so a bundle does not need to carry it. Which other data is derived is stated by the document that owns it (`docs/architecture/execution.md`).

### Mapping onto the hexagon

- **Repository directory** — create and look up organisations and repositories, writing each repository change to the outbox in the same transaction (ADR 0044); deletion is not implemented (§Open questions). Its storage is the metadata database, which is PostgreSQL (ADR 0013, `source-control.md` §Current decisions).
- **Bundle store** — write and read export and backup bundles at a destination chosen by the operator. Possible adapters: local filesystem, S3-compatible store.

## Rationale

- The tenancy model and its signs of being wrong are in ADR 0004.
- Roles, grants and the ownership of the containers: ADR 0021. The shared schema: ADR 0022.
- Shared organisation names, lazy creation in external mode and the directory-change port: ADR 0028 and ADR 0030; the outbox and at-least-once delivery of repository changes: ADR 0044.
- The term "repository" for what ADR 0004 and ADR 0005 call a workspace is in ADR 0020.
- The bundle as the one unit for export, backup and import is in ADR 0005.
- No limit on concurrent landings, and the gate in each process instead of a lock across replicas: ADR 0039, measured in `docs/scale-baseline.md`.

## Open questions

Each is undecided; the recommendation is not a decision.

- **How the base layer composes with a repository.** Options: repository content overlays the base layer by path at read time; the base layer is copied into each repository; a repository pins a base layer version.
  - Recommendation: pin a version and overlay at read time, so a repository's log stays self-describing.
- **Whether a bundle includes the base layer objects a repository references.** Options: include them; reference them by hash and require the organisation's bundle.
  - Recommendation: include them, so a repository restores without data from outside it.
- **Bundle format.** Options: a tar archive of log segments and objects; a directory layout of files; a single custom container.
  - Recommendation: a directory layout of files, for incremental backup to an object store.
- **Incremental scheme.** Options: a cursor on the changeset log plus the objects new since the cursor; a manifest diff of object hashes.
  - Recommendation: a log cursor plus new objects.
- **Metadata contents and versioning.** What the metadata in a bundle contains and how a bundle declares its version. No option examined.
- **Consistency of a backup taken while the repository is live.** Options: snapshot at a log position; stop landing during the backup.
  - Recommendation: snapshot at a log position, since landing holds no lock across server processes.
- **Encryption and integrity of bundles.** Options: hash manifest only; manifest plus encryption chosen by the operator. No recommendation yet.
- **Deleting a repository or organisation**, and whether deletion is recoverable from a bundle. No option examined.
