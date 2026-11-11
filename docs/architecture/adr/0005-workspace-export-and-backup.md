# ADR 0005: A workspace is exported and backed up as a bundle

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The decision was settled in the design session recorded in issue #1 (item 8). With Git dropped (ADR 0003), the repository clone no longer doubles as a copy of the history, so the server holds the only copy unless the system provides one. A workspace is the unit of one codebase (ADR 0004). Issue #1 names no alternative that was rejected.

## Decision

Export and backup are per workspace. The unit is a bundle of the changeset log, the objects it references, and metadata. Backup is incremental. Import reads the same bundle. The shape of the system is in `docs/architecture/workspace.md`.

## Consequences

- A workspace can be moved or restored without the rest of its organisation.
- One bundle serves export, backup and import, so a restore path is exercised by the migration path.
- Data that can be rebuilt from the bundle is not in it; what counts as such is recorded in the responsibility document that owns that data.
- The bundle format, the incremental scheme and what the metadata contains are not decided here.

## Signs the decision was wrong

- A restore from backup fails or loses changesets that landed before the backup.
- Incremental backup grows with the size of the workspace rather than with what changed.
- A workspace cannot be restored without data from outside it, such as the organisation's base layer.
- Import needs information the bundle does not carry.
