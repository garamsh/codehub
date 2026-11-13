# ADR 0045: The server answers unauthenticated liveness and readiness probes outside `/v1`

> Status: accepted
> Date: 2026-11-13

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0024 puts the API under the path prefix `/v1`, has callers present bearer tokens, and maps a backend failure to 500. The server runs as a container (`docs/deployment.md`), and whoever declares its deployment needs two things an orchestrator asks of a process: whether it is alive, and whether it may receive traffic. An orchestrator's probe carries no token, and a path under `/v1` is versioned with the API, which a probe is not part of.

Alternatives not taken:

- **Probes under `/v1`, with a token.** A probe would need a credential the orchestrator does not hold, and a token-issuer outage would mark a healthy server not ready.
- **Probes under `/v1`, without a token.** `/v1` would then hold routes that carry no bearer token and no version of the API, so "everything under `/v1` is authenticated" would stop being true.
- **No readiness probe.** The server would receive traffic before PostgreSQL or the object store answered, and every request would fail as a 500.
- **Readiness answering 500 or 200 with a body.** A probe reads the status, and 503 is the status for a dependency that does not answer.

## Decision

- `GET /healthz` and `GET /readyz` are served at the root, outside `/v1`, and need no token.
- `/healthz` answers 200 while the process serves requests and depends on nothing.
- `/readyz` asks PostgreSQL and the object store, each given 2 seconds to answer. It answers 200 where both answer, and otherwise 503 as an RFC 9457 problem detail whose `detail` names the dependency that did not.
- Both are in the committed OpenAPI document, and the test that asserts every route is documented covers them.
- This sets aside two statements of ADR 0024 for these two routes only: the prefix `/v1`, and bearer tokens. Every other statement of ADR 0024 stands, and a failure reaching any other route is still 500.

## Consequences

- An orchestrator probes the server without a credential.
- The probes disclose to an unauthenticated caller that the server is up and which dependency is down. They disclose nothing about organisations, repositories or users.
- `/readyz` is 503 whenever PostgreSQL or the object store is slow beyond 2 seconds, and a server in that state is taken out of rotation until they answer.
- The probe asks the object store for an object, so a missing bucket answers as a missing object and is not detected.

## Signs the decision was wrong

- Probes are asked for more than reachability, such as a version or a dependency's detail, and begin to disclose what a token should guard.
- Readiness flaps under load because 2 seconds is too short for a healthy dependency, and operators raise or disable it.
