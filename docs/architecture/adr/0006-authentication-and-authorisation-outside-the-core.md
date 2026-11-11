# ADR 0006: Authentication and authorisation are separated from the core and speak standards

> Status: accepted; the agent and Token Exchange parts superseded by ADR-0027
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The decision was settled in the design session recorded in issue #1 (item 3). The system is used by people, by agents and the `ch` CLI acting for them, and by runners. The project owner intends to operate an external identity and authorisation service later, and plugging it in must not change the core. Initial deployment is a single machine, where such a service does not yet exist. Issue #1 fixes the standards. Identity and policy built into the core is the alternative not taken.

## Decision

The core holds no user directory, issues no tokens and evaluates no policy. It reaches identity, authorisation and provisioning only through ports in the application's terms, whose default adapters speak these standards:

- OpenID Connect for people; the core is an OAuth resource server that validates tokens.
- OAuth 2.1 plus OAuth 2.0 Token Exchange (RFC 8693) for agents and the CLI.
- OpenID AuthZEN Authorization API for the decision call from the core, as policy enforcement point, to a policy decision point.
- SCIM 2.0 for provisioning.
- Mutual TLS for runner identity.

The ports are named in `docs/architecture/access.md`.

## Consequences

- Replacing the identity provider, the decision service or the provisioning source is an adapter and composition-site change; the application does not change.
- Every request costs an identity verification and a decision call, or a cached answer to them; the latency and availability of the decision service become the core's.
- The core cannot offer a login of its own; the single-machine stage needs an identity provider, and which one is an open question in `access.md`.
- OAuth 2.1 is an Internet-Draft, not an RFC; the core depends on a specification still changing.
- Authorisation policy and its storage live outside the core; the core defines only what it asks (subject, action, resource, context).

## Signs the decision was wrong

- Making the single-machine deployment usable requires an identity provider or policy engine bundled into the core.
- A needed decision cannot be expressed as an AuthZEN request, so the core carries policy logic or a vendor extension.
- The adapter for a real external service needs the application to change.
- The OAuth 2.1 draft is abandoned, or changes so that the validation the core performs differs from the RFCs it builds on.
- The ports have one adapter each and the tests substitute none of them (the signs in ADR 0001 apply to these ports too).
- Decision-call latency dominates request latency at the target scale of hundreds of concurrent agents, and caching decisions is not safe.
