# Workspace

How the system is divided into organisations and workspaces, and how a workspace is exported, backed up and imported.

## Current decisions

### Tenancy

- **An organisation contains workspaces** (ADR 0004).
- **One workspace is one codebase**, with one changeset log (`docs/architecture/source-control.md`).
- **An organisation holds a base layer** that its workspaces share.
- **Target scale** is hundreds of concurrent agents.
- **Tenancy scopes every port that reaches stored data**: a request names its workspace and, through it, its organisation.
- Authentication and authorisation of people, agents and runners are outside this document (`docs/architecture/access.md`).

### Export and backup

- **Export, backup and import are per workspace** (ADR 0005).
- **The unit is a bundle** of the changeset log, the objects it references, and metadata.
- **Backup is incremental.**
- **Import reads the same bundle** that export and backup write.
- **Derived data is rebuildable** (`docs/architecture/source-control.md`), so a bundle does not need to carry it. Which other data is derived is stated by the document that owns it (`docs/architecture/execution.md`).

### Mapping onto the hexagon

- **Workspace directory** — create, look up and delete organisations and workspaces. Its storage is the metadata database, which is PostgreSQL (ADR 0013, `source-control.md` §Current decisions).
- **Bundle store** — write and read export and backup bundles at a destination chosen by the operator. Possible adapters: local filesystem, S3-compatible store.

## Rationale

- The tenancy model and its signs of being wrong are in ADR 0004.
- The bundle as the one unit for export, backup and import is in ADR 0005.

## Open questions

Each is undecided; the recommendation is not a decision.

- **How the base layer composes with a workspace.** Options: workspace content overlays the base layer by path at read time; the base layer is copied into each workspace; a workspace pins a base layer version.
  - Recommendation: pin a version and overlay at read time, so a workspace's log stays self-describing.
- **Whether a bundle includes the base layer objects a workspace references.** Options: include them; reference them by hash and require the organisation's bundle.
  - Recommendation: include them, so a workspace restores without data from outside it.
- **Bundle format.** Options: a tar archive of log segments and objects; a directory layout of files; a single custom container.
  - Recommendation: a directory layout of files, for incremental backup to an object store.
- **Incremental scheme.** Options: a cursor on the changeset log plus the objects new since the cursor; a manifest diff of object hashes.
  - Recommendation: a log cursor plus new objects.
- **Metadata contents and versioning.** What the metadata in a bundle contains and how a bundle declares its version. No option examined.
- **Consistency of a backup taken while the workspace is live.** Options: snapshot at a log position; stop landing during the backup.
  - Recommendation: snapshot at a log position, since landing has no workspace-wide lock.
- **Encryption and integrity of bundles.** Options: hash manifest only; manifest plus encryption chosen by the operator. No recommendation yet.
- **Deleting a workspace or organisation**, and whether deletion is recoverable from a bundle. No option examined.
- **Per-workspace limits** under the hundreds-of-agents target, such as concurrent landings; no figure is given in issue #1.
