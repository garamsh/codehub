# Architecture

Current architecture of the system and the decisions behind it. This file is the index; workers read it before touching code, the PM keeps it honest.

## Structure

- **Responsibility documents** (one `.md` per domain or concern, e.g. `memory.md`, `gateway.md`) — the single source of truth for how the system is shaped *now*. To know the current state, read only these.
- **`adr/`** — the record of individual decisions: direction taken, context, rejected alternatives.

## Rules

1. **Responsibility document skeleton**: current decisions (consolidated) / rationale summary with links to the relevant ADRs / open questions. Do not copy ADR content — synthesize the present state.
2. **ADRs are append-only.** Once merged, the body is frozen. Exceptions: updating the status field (`accepted` → `superseded by ADR-XXXX`), fixing typos or broken links, and appending a dated entry under `## Errata` when the decision stands but a fact supporting it was wrong — the erratum names what falsified it, and the original text stays intact. A changed decision means a new ADR that supersedes the old one — never an edit.
3. **Decisions land in pairs.** A PR that adds or supersedes an ADR must update the affected responsibility documents in the same PR. A PR with only one of the two is rejected — the final state must always live in the responsibility documents.
4. Keep this index current: every responsibility document and every ADR is listed here. An ADR covers a decision that changes a project rule — a settled choice that does not affect the rules does not earn one.
5. **Every domain or concern has a responsibility document.** The PR that introduces one adds its document; the PR that removes one deletes it. A system with no code yet has `structure.md` (rule 6) and the document for each concern a recorded decision has introduced, and no other; a concern no decision has introduced has none, and that is the correct state.
6. **Structural rules live in `structure.md`.** The shape chosen at bootstrap imposes rules — what the unit is, what may reference what, when a new one is earned — and they belong in the responsibility document for that concern, where a reviewer cites one by line and a correction is an ordinary edit. Not the ADR: rule 2 freezes that body, so superseding the choice of shape would be the price of fixing a rule's wording.
7. **A point the structural rules leave to the project is settled in an ADR** — the one recording the shape where the question is met at bootstrap, a later one where it is met later. Which side is taken decides what counts as a defect, so it is a decision under rule 4, and the ADR still holds the record after bootstrap. Nothing obliges a project to settle a question it has not met: until then the point waits under `structure.md`'s open questions, where it binds nothing, and moves into that document's current decisions when the project takes a side.
8. **Structural rules yield only to what a framework imposes.** Where `structure.md` and a convention file decide the same case differently, `structure.md` governs, unless the stack-specific file that applies decides it as its framework imposes it — the alternative fails to build or run, so the project cannot restate it. A stack file's spelling of a structural rule is not that: the rule it spells is `structure.md`'s. The shape chosen at bootstrap is this project's own refinement of what a stack-neutral file states for every project. A project with no `structure.md` meets no such conflict, and this rule asks nothing of it.

## Index

- `structure.md` — hexagonal structure: what the unit is, what may reference what, when a new port is earned
- `adr/0001-hexagonal-structure.md` — ADR 0001, the system is structured as a hexagon
- `adr/0002-rust-implementation-language.md` — ADR 0002, Rust is the implementation language
- `adr/0014-application-rings.md` — ADR 0014, the application is partitioned into a domain ring and an application ring
- `adr/0018-async-ports.md` — ADR 0018, ports are asynchronous traits dispatched statically, and the domain ring is synchronous
- `adr/0003-server-centred-source-control.md` — ADR 0003, source control is server-centred and replaces Git, with server-side landing
- `adr/0004-organisation-workspace-tenancy.md` — ADR 0004, an organisation contains workspaces and a workspace is one codebase
- `adr/0005-workspace-export-and-backup.md` — ADR 0005, a workspace is exported and backed up as a bundle
- `source-control.md` — source control: objects, changesets, derived data, server-side landing, and the ports they imply
- `adr/0011-source-control-data-model.md` — ADR 0011, objects are SHA-256 addressed, changesets are deltas, and trees and file history are derived
- `adr/0012-refs-and-landing-merge.md` — ADR 0012, a workspace has named refs moved by compare-and-swap, and landing merges at line level
- `adr/0017-landing-merge-details.md` — ADR 0017, the landing merge diffs lines by histogram, tells binary by content, and detects no renames
- `adr/0019-file-kind-and-file-against-directory.md` — ADR 0019, the landing merge merges the file kind apart from content, and a file against a directory conflicts where a delta is applied to a tree
- `adr/0013-metadata-storage.md` — ADR 0013, PostgreSQL holds the metadata, the changeset log and the refs, and blobs stay in the object store
- `adr/0016-object-encoding-and-changeset-identifier.md` — ADR 0016, objects use a project-defined binary encoding, and a changeset is identified by its address
- `tenancy.md` — tenancy (organisation, repository, base layer) and repository export, backup and import
- `adr/0020-repository-replaces-workspace.md` — ADR 0020, a repository is what ADRs 0004 and 0005 call a workspace
- `adr/0021-tenancy-and-access-model.md` — ADR 0021, organisations have admins and members, and members hold path-prefix grants
- `adr/0022-metadata-storage-details.md` — ADR 0022, metadata lives in one shared PostgreSQL schema, migrated forward-only at server start
- `adr/0023-object-storage-details.md` — ADR 0023, objects are keyed per repository, verified by checksum, and uploaded in one part
- `adr/0024-http-api-conventions.md` — ADR 0024, the HTTP API is versioned in the path, described by OpenAPI 3.1 and reports errors as problem details
- `adr/0025-cli-m2-verbs-and-state.md` — ADR 0025, the M2 CLI has a fixed set of verbs, keeps three items of local state and speaks plain HTTP
- `interfaces.md` — the `ch` CLI and the API behind it: what each may do, and what stays server-side
- `adr/0010-cli-and-mcp-interfaces.md` — ADR 0010, the CLI and the MCP server are first-class interfaces over server-side work; the MCP part is superseded by ADR 0026
- `adr/0015-server-api-transport.md` — ADR 0015, the server API is HTTP with JSON described by OpenAPI, and object bytes move by presigned URL
- `access.md` — identity, authorisation and provisioning ports and the standards their default adapters speak
- `adr/0006-authentication-and-authorisation-outside-the-core.md` — ADR 0006, authentication and authorisation are separated from the core and speak standards; the agent and Token Exchange parts are superseded by ADR 0027, and mutual TLS for runners by ADR 0034
- `adr/0026-no-mcp-server-in-codehub.md` — ADR 0026, codehub builds no MCP server, and its first-class interfaces are `ch` and the HTTP API
- `adr/0027-one-kind-of-principal.md` — ADR 0027, codehub knows one kind of principal, and how a token is obtained is the issuer's and the client's
- `adr/0028-federation-through-standards.md` — ADR 0028, codehub is federated through standards, with one audience and shared organisation names
- `adr/0029-identity-verification-jwt-or-introspection.md` — ADR 0029, a token is verified as an OIDC JWT or by RFC 7662 introspection, selected by configuration
- `adr/0030-authorisation-through-authzen-and-external-mode.md` — ADR 0030, authorisation may go through AuthZEN in an external mode, and directory changes are reported through a port; the clause calling a failure a deny is superseded by ADR 0033
- `adr/0031-ch-token-sources-and-exchange.md` — ADR 0031, `ch` reads a token from a file or an environment variable and may exchange it by RFC 8693
- `adr/0032-no-request-body-on-delete.md` — ADR 0032, no HTTP operation of codehub's API takes a request body on DELETE
- `adr/0033-failing-decision-point-is-a-server-error.md` — ADR 0033, a failing decision point fails the request as a server error and allows nothing; supersedes one clause of ADR 0030
- `execution.md` — caches, derived images, delivery through object storage, executor drivers
- `runner.md` — runner registration, polling, labels, pools, job lifecycle
- `adr/0007-cache-model-c.md` — ADR 0007, caches are versioned workspace data with images derived from them
- `adr/0008-image-delivery-and-executor-drivers.md` — ADR 0008, images delivered through object storage, environment known only to executor drivers
- `adr/0009-runner-registration-and-job-protocol.md` — ADR 0009, runners follow the GitHub Actions self-hosted runner model; the registration token, runner credentials and mutual TLS are superseded by ADR 0034
- `adr/0034-runners-are-principals-polling-over-http.md` — ADR 0034, a runner is a registered principal that long-polls the HTTP API and runs jobs on its host
- `adr/0035-post-land-checks-last-green-and-revert.md` — ADR 0035, post-land checks are configured in the repository, move `last-green` and revert a blocking failure
