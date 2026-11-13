# ADR 0004: An organisation contains workspaces, and a workspace is one codebase

> Status: accepted; the term "workspace" superseded by ADR-0020
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The decision was settled in the design session recorded in issue #1 (item 2). The system serves several codebases and several groups of users, with hundreds of concurrent agents as the target scale. Codebases belonging to one organisation share material, such as common code. Issue #1 names no alternative tenancy model that was rejected.

## Decision

An organisation contains workspaces. One workspace is one codebase and has one changeset log. An organisation holds a base layer shared by its workspaces. The shape of the system is in `docs/architecture/tenancy.md`.

## Consequences

- The workspace is the unit that source control, export and backup operate on.
- The changeset log is per workspace, so ordering and landing are scoped to a workspace and workspaces do not contend with one another.
- The base layer is organisation-level data that more than one workspace reads; how it composes with a workspace is not decided here.
- Every request, object and job belongs to a workspace and through it to an organisation; the tenancy path is part of every port that reaches stored data.
- Who may do what inside an organisation or workspace is decided by the access design, not here (`docs/architecture/access.md`).

## Signs the decision was wrong

- A single codebase needs more than one independent changeset log, or two workspaces need one log.
- The base layer cannot express what organisations share, and workspaces copy it instead.
- Workspaces in one organisation need a unit of work spanning more than one of them, and landing cannot express it.
- Hundreds of concurrent agents in one workspace overload its single log.
