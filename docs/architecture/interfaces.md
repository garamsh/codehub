# Interfaces

The ways clients reach the system, which are the `ch` CLI, the MCP server and the API behind them, and what each may and may not do.

## Current decisions

- **Two first-class interfaces** (ADR 0010): a minimal CLI named `ch`, and an MCP server. Agents are the main clients; people use the CLI.
- **Both are driving adapters** under `structure.md`. Each translates between its protocol and the application's driving ports, and holds no subject matter of its own. An operation is added to a port first; an interface then exposes it.
- **Complex work is server-side.** An interface hands a request to the server and reports the outcome. It does not orchestrate multi-step work, and it does not retry or compensate around the server's own steps. The server-side landing automation is described in `source-control.md`.
- **The client is kept to a few verbs.** GitHub's workflow and CLI grew too complex and this project does not carry that over. The verb list is not fixed; see open questions.
- **The CLI is named `ch`.** `hub` was rejected because Debian ships a package of that name, GitHub's old CLI.
- **The server API is HTTP with JSON, described by OpenAPI** (ADR 0015). The CLI and the MCP server call it; its bodies carry metadata, not object bytes.
- **Object bytes move by presigned URL** (ADR 0015). The API issues a short-lived presigned URL and the client uploads or downloads the bytes directly to or from the S3-compatible object store, as runners already fetch images (`execution.md`, ADR 0008).
- **Credentials are not decided here.** How the CLI and agents obtain and present them is in `access.md`; interfaces carry what it defines.

## Rationale

- Why two first-class interfaces over server-side work: ADR 0010.
- Why interfaces are adapters over shared ports: ADR 0001 and `structure.md`.
- Why HTTP with JSON and presigned URLs for object bytes, and what would show it wrong: ADR 0015. It was decided before authentication on the API and the runner-to-server transport were settled, although this document's earlier open question tied the choice to them; ADR 0015 names a second transport added on the same endpoint for either of them as a sign the decision was wrong.
- MCP facts the open questions rely on, checked on 2026-11-11 against modelcontextprotocol.io, pages `/specification/latest/basic/transports` and `/specification/latest/basic/versioning`:
  - The latest revision there is `2026-07-28`. Revisions `2025-11-25` and earlier are called legacy and use an `initialize` handshake.
  - The standard transports are stdio and Streamable HTTP.
  - Which revisions and transports this system serves is not decided.

## Open questions

Each is settled in an ADR when the project takes a side (`docs/architecture/README.md` rule 4).

- **Verb list of `ch`.** Options: a fixed small set; a small set plus an alias or plugin mechanism; one verb per server operation. Recommendation: a fixed small set, sketched as `login`, `use <org>/<ws>`, `find`, `read`, `edit` or `put`, `land`, `log`, `run`. The sketch was discussed and is not decided, including whether `edit` and `put` are one verb.
- **OpenAPI version.** Options: OpenAPI 3.0; OpenAPI 3.1. Recommendation: 3.1, since its Schema Object is JSON Schema 2020-12 (OpenAPI Specification v3.1.0), the dialect MCP defaults to (modelcontextprotocol.io, `/specification/latest/basic`, revision `2026-07-28`, section JSON Schema Usage), so one schema can serve both.
- **API versioning scheme.** Options: a version in the path; a version in a header or media type; no version, with additive changes only. Recommendation: none yet; it depends on how clients are released, which is not decided.
- **Streaming and long-running operations**, such as reporting a landing and its post-land checks. Options: long-poll; server-sent events; a status resource the client polls. Recommendation: none yet; ADR 0010 counts repeated polling around one workflow as a sign of a wrong unit of work, so the choice is made per operation with that in view.
- **Whether runners use the same endpoint.** Options: runners call the same HTTP JSON API; runners use a separate endpoint and transport (`runner.md`, runner-to-server transport). Recommendation: none yet; ADR 0015 does not decide it, and runner identity is mutual TLS (`access.md`), which may favour a separate listener.
- **MCP revisions and transports served.** Options: the latest revision only; the latest plus legacy revisions; stdio, Streamable HTTP, or both. Recommendation: the latest revision over Streamable HTTP for the hosted server, and stdio only if a client needs a local process.
- **Local state in the CLI.** Options: none beyond the credential; the credential plus the selected `<org>/<ws>`; a local cache of read objects. Recommendation: the credential and the selected workspace only, since any further local state is a copy of server data that can go stale.
- **Output format for agents.** Options: human text only; JSON on a flag; JSON by default when stdout is not a terminal. Recommendation: human text by default and JSON on a flag, with the MCP server returning structured results built from the same port types.
