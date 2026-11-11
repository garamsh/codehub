# ADR 0026: codehub builds no MCP server, and its first-class interfaces are `ch` and the HTTP API

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0010 made a minimal CLI and an MCP server the first-class interfaces. Goal issue #64 (decision 1) revisits the MCP half. Every operation an MCP server would offer is already an operation of the HTTP API described by OpenAPI (ADR 0015, ADR 0024), so the MCP server would be a second adapter to keep in step with the CLI and the API. It would also need its own authorisation handling, since the MCP authorization revision makes an MCP server an OAuth resource server, which repeats the identity decisions of ADR 0028 and ADR 0029 for a second front.

Alternatives not taken:

- Building the MCP server in codehub, as ADR 0010 decided. A second adapter with its own transports and protocol revisions to follow, for clients that can already call HTTP.
- Generating an MCP server from the OpenAPI document and shipping it with codehub. It is still a surface to version and secure, and the project would own its compatibility with a protocol that revises.

## Decision

- The first-class interfaces are the `ch` CLI and the HTTP API described by OpenAPI. Neither is secondary to the other.
- codehub builds no MCP server. Anyone may build one, outside codehub, over the HTTP API.
- The rest of ADR 0010 stands: complex work is done on the server, and the interfaces are driving adapters over the same driving ports.
- This supersedes the MCP part of ADR 0010.

## Consequences

- One adapter fewer to keep in step with the application's driving ports.
- An agent that wants MCP depends on a server someone else maintains; its availability and fit are not codehub's.
- The open question in `docs/architecture/interfaces.md` about MCP revisions and transports is removed.
- Credentials for the API are decided once, in `docs/architecture/access.md`, with no second resource server to configure.

## Signs the decision was wrong

- Several outside MCP servers over the API appear and each re-implements workflow steps the server does not expose, so they disagree.
- Agent clients that matter cannot call HTTP or run the CLI, so they cannot use codehub without an MCP server.
- The API grows operations only to suit MCP clients' tool model.
