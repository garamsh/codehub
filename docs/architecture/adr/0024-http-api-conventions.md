# ADR 0024: The HTTP API is versioned in the path, described by OpenAPI 3.1 and reports errors as problem details

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0015 chose HTTP with JSON described by OpenAPI. Left open in `docs/architecture/interfaces.md` were the OpenAPI version and the versioning scheme; how errors and access failures are reported was not recorded. ADR 0021 defines who may do what.

Alternatives not taken:

- OpenAPI 3.0. Its Schema Object is not JSON Schema 2020-12, the dialect MCP defaults to, so one schema could not serve both.
- A version in a header or media type, or no version with additive changes only. A path prefix is visible in logs and to any HTTP client, and the project has no client release process that would make unversioned additive change safe.
- A bespoke error body. RFC 9457 is a standard clients may already parse.
- 403 for a repository the caller may not read. It discloses that the repository exists.

## Decision

- The API is described by OpenAPI 3.1, under the path prefix `/v1`. Errors are RFC 9457 problem details. Callers present bearer tokens.
- Resources live under `/v1/orgs/{org}/repos/{repo}/…`.
- Status codes: 401 without a valid token; 404 when the caller may not read the repository, so its existence is not disclosed; 403 when the caller may read it but not perform the action; 409 for a landing conflict or a lost compare-and-swap, naming the paths.
- The OpenAPI document is committed in `crates/adapter-http` and served at `/v1/openapi.json`. A test asserts that the two are equal.
- Landing is a synchronous request in M2; streaming stays open.
- The server binary is `codehub`. It binds `127.0.0.1` unless configured otherwise.

## Consequences

- The committed document is what clients rely on, and a change to the served API that is not in the document fails the test.
- A breaking change is a `/v2` prefix beside `/v1`.
- A landing holds its request open for its duration, which bounds it by client and proxy timeouts.
- A caller cannot tell a missing repository from one it may not read.
- A server started without configuration is reachable only from its own machine.

## Signs the decision was wrong

- Landings exceed request timeouts, and clients poll or retry around them.
- The committed document and the served API drift in ways the equality test does not catch.
- Clients need to negotiate versions per request rather than per prefix.
- Callers with read access to an organisation need to distinguish a missing repository, and 404 forces a second lookup.
