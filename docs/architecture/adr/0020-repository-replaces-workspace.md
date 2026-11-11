# ADR 0020: A repository is what ADRs 0004 and 0005 call a workspace

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0004 names the unit inside an organisation a workspace, and ADR 0005 exports and backs it up under that name. The word collides with the Cargo workspace the code lives in (ADR 0002) and with Bitbucket's organisation-level workspace, and it leaves no word for an agent's working area, which a later decision may need. The alternatives were keeping the term and renaming the later concepts, and a different new term; "repository" is what the unit is, one codebase with one changeset log.

## Decision

The unit that an organisation contains, one codebase with one changeset log, is called a repository. This supersedes the term "workspace" in ADR 0004 and ADR 0005; their decisions stand unchanged. The CLI addresses a repository as `<org>/<repo>`, and the port that creates, looks up and deletes organisations and repositories is the repository directory.

## Consequences

- The responsibility documents use "repository"; the frozen bodies of ADRs 0001 to 0019 keep "workspace" and read as "repository" for the unit.
- `workspace.md` is `tenancy.md`, which covers tenancy, export and backup, and the mapping onto the hexagon.
- "Workspace" now means only the Cargo workspace.
- The word "repository" also names a Git repository elsewhere; this project replaces Git (ADR 0003), so the two do not meet inside it.
- The code carries the old name until it is renamed to match; that is a separate change.

## Signs the decision was wrong

- Readers of the documents or the CLI take "repository" for a Git repository, or ask for a Git remote.
- A later concept needs the same word, such as a store of many codebases, and is forced to a worse one.
