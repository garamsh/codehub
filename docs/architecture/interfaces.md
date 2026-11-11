# Interfaces

The ways clients reach the system, which are the `ch` CLI, the MCP server and the API behind them, and what each may and may not do.

## Current decisions

- **Two first-class interfaces** (ADR 0010): a minimal CLI named `ch`, and an MCP server. Agents are the main clients; people use the CLI.
- **Both are driving adapters** under `structure.md`. Each translates between its protocol and the application's driving ports, and holds no subject matter of its own. An operation is added to a port first; an interface then exposes it.
- **Complex work is server-side.** An interface hands a request to the server and reports the outcome. It does not orchestrate multi-step work, and it does not retry or compensate around the server's own steps. The server-side landing automation is described in `source-control.md`.
- **The client is kept to a few verbs.** GitHub's workflow and CLI grew too complex and this project does not carry that over. The verb list is not fixed; see open questions.
- **The CLI is named `ch`.** `hub` was rejected because Debian ships a package of that name, GitHub's old CLI.
- **Credentials are not decided here.** How the CLI and agents obtain and present them is in `access.md`; interfaces carry what it defines.

## Rationale

- Why two first-class interfaces over server-side work: ADR 0010.
- Why interfaces are adapters over shared ports: ADR 0001 and `structure.md`.
- MCP facts the open questions rely on, checked on 2026-11-11 against modelcontextprotocol.io, pages `/specification/latest/basic/transports` and `/specification/latest/basic/versioning`:
  - The latest revision there is `2026-07-28`. Revisions `2025-11-25` and earlier are called legacy and use an `initialize` handshake.
  - The standard transports are stdio and Streamable HTTP.
  - Which revisions and transports this system serves is not decided.

## Open questions

Each is settled in an ADR when the project takes a side (`docs/architecture/README.md` rule 4).

- **Verb list of `ch`.** Options: a fixed small set; a small set plus an alias or plugin mechanism; one verb per server operation. Recommendation: a fixed small set, sketched as `login`, `use <org>/<ws>`, `find`, `read`, `edit` or `put`, `land`, `log`, `run`. The sketch was discussed and is not decided, including whether `edit` and `put` are one verb.
- **Server API transport** behind the interfaces. Options: gRPC; HTTP with JSON. Recommendation: none yet; the choice follows what authentication and the runner protocol need on the same endpoint.
- **MCP revisions and transports served.** Options: the latest revision only; the latest plus legacy revisions; stdio, Streamable HTTP, or both. Recommendation: the latest revision over Streamable HTTP for the hosted server, and stdio only if a client needs a local process.
- **Local state in the CLI.** Options: none beyond the credential; the credential plus the selected `<org>/<ws>`; a local cache of read objects. Recommendation: the credential and the selected workspace only, since any further local state is a copy of server data that can go stale.
- **Output format for agents.** Options: human text only; JSON on a flag; JSON by default when stdout is not a terminal. Recommendation: human text by default and JSON on a flag, with the MCP server returning structured results built from the same port types.
