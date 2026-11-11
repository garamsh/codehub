# ADR 0021: Organisations have admins and members, and members hold path-prefix grants

> Status: accepted; the agent and Token Exchange parts superseded by ADR-0027
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The decisions come from goal issue #33 (decisions 3 to 8) and issue #35. ADR 0004 fixed organisations and repositories, and ADR 0006 fixed that authentication and authorisation sit outside the core and speak standards. Left open in `docs/architecture/access.md` were how directory-level permissions are modelled and what the default decision point is; `docs/architecture/interfaces.md` left open how the CLI obtains a token.

Alternatives not taken:

- The core sends only the repository and the policy engine holds path rules. Path rules would live in a second place the core cannot test against.
- Directory permissions held as repository data the core evaluates, with the engine asked only about the repository. Policy would be split between the core and the engine.
- An engine such as OpenFGA or Cedar as the default decision point now. It adds a service to run before the permission model has been used.
- codehub reading an outside service's organisation mapping. An outside service (garam) depends on codehub, not the reverse, so codehub keeps the tenancy containers and the outside service keeps its own mapping onto them.
- Token Exchange (RFC 8693) for the CLI's person token. It serves agents acting for a person; a person at a terminal has no token to exchange.

## Decision

- Any authenticated principal may create an organisation and becomes its admin. Roles are `admin`, who may take every action on the organisation and its repositories, and `member`.
- A member holds grants, each a repository, a path prefix and `read` or `write`. `write` includes `read`. A prefix matches by whole components: `src` covers `src/a` and not `srcx/a`. The empty prefix is the whole repository.
- Organisation admins create repositories and add or remove members and grants.
- codehub owns organisations and repositories as tenancy containers. An outside service keeps its own mapping onto them and depends on codehub.
- The core sends the path as a resource attribute to the authorisation decision port. `land` asks for `write` on every path the draft changes and rejects the whole landing, naming every denied path. `read` and `log` ask for `read` on their path.
- The default authorisation adapter keeps memberships and grants in PostgreSQL and evaluates them there. Administering them is a driven port of its own, the membership port. The AuthZEN adapter of ADR 0006 replaces the default adapter later.
- The CLI obtains a person's token with the OAuth 2.0 Device Authorization Grant (RFC 8628). Token exchange for agents (ADR 0006) is unchanged and not yet implemented.

## Consequences

- Path permissions are enforced in one place, the decision port, and a landing that touches a denied path fails whole and tells the caller which paths.
- The default adapter needs no policy service beside PostgreSQL; moving to an engine reachable through AuthZEN is an adapter change.
- A new port, membership, has a PostgreSQL adapter and a test adapter (`docs/architecture/structure.md`).
- Listing a directory returns entries the caller may not read unless it is filtered; that question stays open.
- Roles are two, so a read-only organisation role or a repository-level admin is not expressible.

## Signs the decision was wrong

- Grants need an inheritance, a deny or a group that prefix grants cannot express, and the default adapter grows into a policy engine.
- Per-request grant evaluation in PostgreSQL shows in landing latency at the target of hundreds of concurrent agents.
- Two roles do not cover who administers a repository, and a third role or repository-level admins are asked for repeatedly.
- The outside service needs codehub to know its mapping, which reverses the dependency.
