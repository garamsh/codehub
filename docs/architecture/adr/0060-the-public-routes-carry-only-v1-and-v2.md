# ADR 0060: the public routes carry `/v1/` and `/v2/` only

> Status: accepted
> Date: 2026-11-23

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The `Ingress` of ADR 0057 and the `HTTPRoute` of ADR 0058 send every path that is not `/v1/runner/` to `api`. A server answers `GET /healthz`, `GET /readyz` and `GET /metrics` unauthenticated and outside `/v1` (ADR 0045, ADR 0056), so a route that names `/` makes the metrics, which carry organisation and repository names in their labels, readable from the public host. Goal #316 puts the `HTTPRoute` on `codehub.garam.sh`.

Choices considered and not taken:

- **Metrics on a port of their own.** The server serves the probes and `/metrics` on its one listening address (ADR 0045); a second listener is a change to every role for a path a route can leave out.
- **A rule that answers `/metrics` with a 404 in the route.** The Ingress and the HTTPRoute would each need a way to say it, and a path added later is public until someone lists it.
- **Moving `/metrics` under `/v1` with a token.** Rejected by ADR 0056.

## Decision

- **The route for `api` matches `/v1/`** and not `/`, in the `Ingress` and in the `HTTPRoute`. `/v1/runner/` still goes to `runner-api` and the registry's route still takes `/v2/` only.
- **Nothing outside `/v1/` and `/v2/` is routed.** The probes and `/metrics` stay inside the cluster, where the kubelet and the Prometheus scrape reach the pods directly.
- **A chart assertion** (`make helm-check`) renders the routes and fails where a path prefix of any route covers `/metrics`.

## Consequences

- A path the server adds outside `/v1` is not public until the chart names it. That is the intent.
- Supersedes the clause of ADR 0058 that sends "every other path" to `api`, and the same split of ADR 0057's `Ingress`.

## Signs the decision was wrong

- A client needs a public path outside `/v1/` and `/v2/`, such as a well-known document served by the server.
