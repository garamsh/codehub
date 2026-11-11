# ADR 0015: The server API is HTTP with JSON described by OpenAPI, and object bytes move by presigned URL

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The decision was settled in the design session recorded in issue #12 (decision 10). The `ch` CLI and the MCP server are driving adapters over the server (ADR 0010), and `docs/architecture/interfaces.md` listed the transport of the API behind them as open, with gRPC and HTTP with JSON as the options.

- MCP messages are JSON-RPC 2.0 (modelcontextprotocol.io, `/specification/latest/basic`, revision `2026-07-28`, section Messages), and the CLI's agent output is planned as JSON (`docs/architecture/interfaces.md`, open question on output format). Both clients already handle JSON.
- The heavy traffic is object bytes. The S3-compatible object store holds source-control objects (`docs/architecture/source-control.md`, Object store port) and filesystem images, and images already reach runners through short-lived presigned URLs the server issues (ADR 0008, `docs/architecture/execution.md` §Delivery). Bytes that move between client and object store do not cross the API, so a binary RPC framing's throughput advantage applies to little of what the API carries.
- The open question said the choice follows what authentication and the runner protocol need on the same endpoint. Neither was settled when this decision was taken: how credentials reach the API beyond the standards in `docs/architecture/access.md`, and the runner-to-server transport (`docs/architecture/runner.md`, open questions), are still open.

gRPC was not taken.

## Decision

- The server API is HTTP with JSON bodies, described by an OpenAPI document.
- Object bytes do not pass through the API. The API issues presigned URLs, and the client uploads and downloads object bytes directly to and from the object store, as ADR 0008 already does for images.

## Consequences

- The MCP server and the CLI translate JSON to JSON; neither carries a binary codec or generated stubs for a second wire format.
- The API can be called with any HTTP client, which makes it inspectable by hand and from tests.
- The API's request and response bodies stay small, so its throughput is bounded by metadata operations, not by object size.
- The object store becomes an availability dependency of every client that reads or writes objects, not only of runners.
- Authorisation for object bytes is a URL the server issues, so the server decides who may read or write an object, and a URL that leaks grants access until it expires.
- The API's schema is the OpenAPI document; keeping it and the driving ports in step is work the project takes on.
- Server-to-client streaming has no native form in HTTP with JSON; an operation that needs it uses long-poll, server-sent events or a status resource, chosen per operation.

## Signs the decision was wrong

- The runner protocol or authentication needs a transport on the same endpoint that HTTP with JSON cannot carry, so a second API transport is added beside this one.
- Object bytes start passing through the API because presigned URLs do not fit a client or an object store adapter, so the API carries the heavy traffic this decision assumed it would not.
- Measured API time is dominated by JSON encoding or decoding, or by per-request HTTP overhead, at the target of hundreds of concurrent agents.
- Operations that report progress (a landing and its checks, for example) are polled so often that clients and server spend more on polling than on the work, and a streaming transport is introduced to replace it.
- The OpenAPI document and the server drift apart repeatedly, so the description stops being the schema clients rely on.
