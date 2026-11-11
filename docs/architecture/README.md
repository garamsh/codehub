# Architecture

Current architecture of the system and the decisions behind it. This file is the index; workers read it before touching code, the PM keeps it honest.

## Structure

- **Responsibility documents** (one `.md` per domain or concern, e.g. `memory.md`, `gateway.md`) — the single source of truth for how the system is shaped *now*. To know the current state, read only these.
- **`adr/`** — the record of individual decisions: direction taken, context, rejected alternatives.

## Rules

1. **Responsibility document skeleton**: current decisions (consolidated) / rationale summary with links to the relevant ADRs / open questions. Do not copy ADR content — synthesize the present state.
2. **ADRs are append-only.** Once merged, the body is frozen. Exceptions: updating the status field (`accepted` → `superseded by ADR-XXXX`), fixing typos or broken links, and appending a dated entry under `## Errata` when the decision stands but a fact supporting it was wrong — the erratum names what falsified it, and the original text stays intact. A changed decision means a new ADR that supersedes the old one — never an edit.
3. **Decisions land in pairs.** A PR that adds or supersedes an ADR must update the affected responsibility documents in the same PR. A PR with only one of the two is rejected — the final state must always live in the responsibility documents.
4. Keep this index current: every responsibility document and every ADR is listed here. `structures/` is not — bootstrap deletes it, and an entry for a file that is about to go leaves the index wrong. An ADR covers a decision that changes a project rule — a settled choice that does not affect the rules does not earn one.
5. **Every domain or concern has a responsibility document.** The PR that introduces one adds its document; the PR that removes one deletes it. A system with no code yet has no more than `structure.md` (rule 6), and that is the correct state.
6. **Structural rules live in `structure.md`.** The shape chosen at bootstrap imposes rules — what the unit is, what may reference what, when a new one is earned — and they belong in the responsibility document for that concern, where a reviewer cites one by line and a correction is an ordinary edit. Not the ADR: rule 2 freezes that body, so superseding the choice of shape would be the price of fixing a rule's wording.
7. **A point the structural rules leave to the project is settled in an ADR** — the one recording the shape where the question is met at bootstrap, a later one where it is met later. Which side is taken decides what counts as a defect, so it is a decision under rule 4, and the ADR still holds the record after bootstrap. Nothing obliges a project to settle a question it has not met: until then the point waits under `structure.md`'s open questions, where it binds nothing, and moves into that document's current decisions when the project takes a side.
8. **Structural rules yield only to what a framework imposes.** Where `structure.md` and a convention file decide the same case differently, `structure.md` governs, unless the stack-specific file that applies decides it as its framework imposes it — the alternative fails to build or run, so the project cannot restate it. A stack file's spelling of a structural rule is not that: the rule it spells is `structure.md`'s. The shape chosen at bootstrap is this project's own refinement of what a stack-neutral file states for every project. A project with no `structure.md` meets no such conflict, and this rule asks nothing of it.

## Index

- `structure.md` — hexagonal structure: what the unit is, what may reference what, when a new port is earned
- `adr/0001-hexagonal-structure.md` — ADR 0001, the system is structured as a hexagon
- `adr/0002-rust-implementation-language.md` — ADR 0002, Rust is the implementation language
- `adr/0003-server-centred-source-control.md` — ADR 0003, source control is server-centred and replaces Git, with server-side landing
- `adr/0004-organisation-workspace-tenancy.md` — ADR 0004, an organisation contains workspaces and a workspace is one codebase
- `adr/0005-workspace-export-and-backup.md` — ADR 0005, a workspace is exported and backed up as a bundle
- `source-control.md` — source control: objects, changesets, derived data, server-side landing, and the ports they imply
- `workspace.md` — tenancy (organisation, workspace, base layer) and workspace export, backup and import
- `interfaces.md` — the `ch` CLI, the MCP server and the API behind them: what each may do, and what stays server-side
- `adr/0010-cli-and-mcp-interfaces.md` — ADR 0010, the CLI and the MCP server are first-class interfaces over server-side work
- `access.md` — identity, authorisation and provisioning ports and the standards their default adapters speak
- `adr/0006-authentication-and-authorisation-outside-the-core.md` — ADR 0006, authentication and authorisation are separated from the core and speak standards
- `execution.md` — caches, derived images, delivery through object storage, executor drivers
- `runner.md` — runner registration, polling, labels, pools, job lifecycle
- `adr/0007-cache-model-c.md` — ADR 0007, caches are versioned workspace data with images derived from them
- `adr/0008-image-delivery-and-executor-drivers.md` — ADR 0008, images delivered through object storage, environment known only to executor drivers
- `adr/0009-runner-registration-and-job-protocol.md` — ADR 0009, runners follow the GitHub Actions self-hosted runner model
