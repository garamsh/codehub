# ADR 0038: A request asks one decision for all its paths, through AuthZEN Access Evaluations where the decision point offers it

> Status: accepted; the bullet on when the AuthZEN adapter sends Access Evaluations is superseded by ADR 0041
> Date: 2026-11-12

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

A landing asks for `write` on every path it changes (ADR 0021), and the core asks the decision port once per path. ADR 0030 decides no caching of decisions, and `access.md` left open whether to batch evaluations, to be measured first.

The scale baseline (`docs/scale-baseline.md`) measured 11.18 decision-point calls per API request in external mode, with 20 paths per landing.

OpenID Authorization API 1.0 (AuthZEN), Final, defines an Access Evaluations API: one request carrying many evaluations, answered with one decision each, in order. A decision point advertises it as `access_evaluations_endpoint` in its metadata at `/.well-known/authzen-configuration`. Cerbos 0.57.0, the decision point of `compose.yaml`, advertises it and answers there.

Alternatives not taken:

- **Cache decisions.** ADR 0030 rejected caching, because a revocation would then take effect late, by a delay the core does not bound.
- **Ask for the longest common prefix instead of each path.** A grant on a deeper prefix would be refused, so the answer would differ from the per-path one.
- **Require the batch endpoint.** A decision point that implements only Access Evaluation would stop working.

## Decision

- The authorisation decision port decides one action of one subject on many resources in one call, and answers a decision per resource.
- A request that needs decisions on many paths, such as a landing, makes that call once.
- The AuthZEN adapter reads the decision point's metadata. Where `access_evaluations_endpoint` is advertised, it sends one Access Evaluations request. Where it is not, it sends one Access Evaluation per resource.
- A failing metadata read, a failing batch request, or a batch answer whose number of decisions differs from the number of resources fails the request as a server error and allows nothing (ADR 0033).
- The PostgreSQL and in-memory adapters answer the call by deciding each resource.

## Consequences

- Decision-point calls per landing no longer grow with the number of paths when the decision point offers the batch endpoint.
- The per-path rule of ADR 0021 is unchanged: every path is still decided, and every denied path is named.

## Signs the decision was wrong

- Decision points in use do not advertise the batch endpoint, so the fallback is the common case and the port's added operation buys nothing.
- One batch request's latency at the decision point grows with the number of resources faster than the calls it replaces.
