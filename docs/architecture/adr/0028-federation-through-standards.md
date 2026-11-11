# ADR 0028: codehub is federated through standards, with one audience and shared organisation names

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0006 put identity and authorisation outside the core behind ports. Goal issue #64 (decision 3) settles how an outside layer joins codehub. The first consumer is garam (garamsh/garam#1754, cited as context only), which is to become an OpenID Connect issuer and a policy service and to play for codehub the part an IAM service plays for the services built on it. Each decision here stands without garam: any layer that speaks the standards named in ADR 0029 and ADR 0030 can take that part.

Alternatives not taken:

- A mapping table in codehub between its organisations and the outside layer's. codehub would store the outside layer's model and the two would drift.
- One audience per organisation or per repository. A token would have to be obtained for each, and an issuer would need to know codehub's tenancy.
- Claim mapping in the core, so tokens carry roles or grants. Policy would be split between the issuer and the decision point.

## Decision

- codehub speaks standards only, and the application names no external product. An outside identity and authorisation layer stands to codehub as an IAM service to a cloud service: it issues the tokens and decides.
- codehub accepts only tokens bound to its audience. The audience is one per codehub deployment, a configured string. Scope is not carried in the token; the decision call resolves it.
- The organisation name is shared with the outside layer, with no mapping table. An organisation and its repositories are named the same on both sides.
- A path is sent to the decision point as a resource attribute (ADR 0021).

## Consequences

- Moving to another layer that speaks the standards is a configuration change, not a code change.
- The audience string is a deployment setting both sides must agree on; a mismatch rejects every token.
- A token for another audience is rejected even when its signature is valid.
- An organisation cannot be renamed on one side alone, since the name is the shared key.
- This settles the open question "Token audience and scope" in `docs/architecture/access.md` in the form of its recommendation.

## Signs the decision was wrong

- An issuer cannot mint a token for one configured audience string, so codehub needs several.
- Decisions need token claims the decision point cannot read from its own entities.
- Organisation names collide or must change, and the outside layer then needs a mapping to follow them.
