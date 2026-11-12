# ADR 0030: Authorisation may go through AuthZEN in an external mode, and directory changes are reported through a port

> Status: accepted; the clause calling a failing decision point a deny superseded by ADR-0033; its one configured bearer token superseded by ADR-0042 for the `private_key_jwt` mode; the path as a resource property superseded by ADR-0043 in the `grn` profile; at-most-once delivery of repository changes and the rejected durable queue superseded by ADR-0044
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0006 named the OpenID AuthZEN Authorization API as the decision call, and ADR 0021 kept grants in PostgreSQL as the default adapter with the AuthZEN adapter to come. Goal issue #64 (decisions 5 and 6) adds that adapter. When an outside layer decides, codehub's own members and grants would be a second source of truth, and the layer needs to learn which organisations and repositories exist.

Alternatives not taken:

- Keeping members and grants in PostgreSQL and also asking the decision point. Two places decide, and an allow in one overrides a deny in the other.
- Caching decisions. A revocation then takes effect late, by a delay the core does not bound.
- Allowing when the decision point is unreachable. A stopped decision point would grant every caller access.
- Mapping token claims to roles in the core. The decision point reads its own entities.
- At-least-once delivery of directory changes through a durable queue. It needs a store and a retry process before any adapter needs it.

## Decision

- An AuthZEN 1.0 Access Evaluation adapter, over HTTP, is a second adapter of the authorisation decision port. Configuration selects it or the PostgreSQL default. The adapter authenticates to the decision point with a configured bearer token obtained by OAuth client credentials.
- The request maps Subject, Action and Resource in the application's terms. The path is a resource property.
- On an unreachable or failing decision point the decision is deny, with no caching.
- Selecting the AuthZEN adapter is the external mode. In it:
  - the core keeps no members or grants;
  - the member endpoints are refused;
  - creating an organisation writes no admin grant;
  - an organisation is created lazily on first use when `CreateOrganisation` is allowed, and not created when it is denied.
- A driven port, the directory-change port, reports organisation and repository creation and deletion, so an outside layer can learn of them. M3 has an in-process adapter and a no-op, and no other adapter. Delivery is at most once.

## Consequences

- With the AuthZEN adapter the decision point's availability bounds every request; a stopped decision point denies everything.
- A change lost between the core and an outside layer, because delivery is at most once, is not retried: the outside layer can lack an organisation or repository until a later adapter reconciles it.
- Lazy creation ties an organisation's existence to the first allowed call, so a mistyped name from an allowed caller creates an organisation.
- A new driven port has an in-process adapter used by tests (`docs/architecture/structure.md`).
- This settles two open questions in `docs/architecture/access.md` in the form of their recommendations: "Behaviour when the decision service is unreachable" (deny), and the caching half of "Decision caching and batching" (no caching).

## Signs the decision was wrong

- A needed decision cannot be expressed as Subject, Action, Resource and the path property without a vendor extension.
- Denying on an outage makes the decision point's downtime the project's, and operators ask for a bounded cache.
- An outside layer misses organisations or repositories because delivery is at most once.
- External mode needs members or grants kept in the core after all.
