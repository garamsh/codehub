# ADR 0010: The CLI and the MCP server are first-class interfaces over server-side work

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The decision was settled in the design session recorded in issue #1 (item 7). GitHub's workflow and CLI grew complex, and this project discards that model. The main clients are agents, with people second, and the target is hundreds of concurrent agents. A client that carries workflow logic (rebase, retry, conflict handling, post-land verification) has to be reimplemented for every interface and races with every other client. Two alternatives were not taken: a rich CLI that orchestrates the workflow locally, and a single interface with the other generated from it.

The CLI is named `ch`. `hub` was rejected because Debian ships a package named `hub`, GitHub's command-line wrapper for git (`apt-cache show hub` on Debian trixie, version 2.14.2~ds1-1).

## Decision

- A minimal CLI named `ch` and an MCP server are the first-class interfaces. Neither is secondary to the other or to any API behind them.
- Complex work is done on the server, not by the client. The interfaces hand a request to the server and report the outcome; they do not orchestrate it.
- Both interfaces are driving adapters over the same driving ports (`docs/architecture/structure.md`), and neither holds a copy of the subject matter.

## Consequences

- A behaviour change to a workflow is made once, in the application, and reaches the CLI and the MCP server together.
- The CLI stays small enough that its verb list can be read in one screen.
- Every operation either interface offers needs a driving port in the application's terms; an operation that cannot be named without naming the CLI or MCP is not ready to be offered.
- The server is on the path of every workflow, so a client cannot complete one alone.
- Server availability and latency bound what an agent can do.

## Signs the decision was wrong

- A workflow step is implemented in the CLI or the MCP server because the server operation does not expose it, and the two adapters then disagree.
- The CLI's verb list grows to mirror the server's operations one for one.
- Clients repeatedly poll or retry around a server operation to complete one workflow, which means the unit of server-side work was drawn too small.
- Agents or people reach the system through a third interface because neither first-class one serves them.
