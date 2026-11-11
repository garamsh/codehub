# ADR 0027: codehub knows one kind of principal, and how a token is obtained is the issuer's and the client's

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0006 and ADR 0021 distinguish people from agents: the identity port returns "a person, or an agent or the CLI acting for a person", agents are to hold tokens obtained by OAuth 2.0 Token Exchange (RFC 8693) at the authorisation server, and the CLI's person token comes from the Device Authorization Grant (RFC 8628). Nothing in the core acts differently on that distinction: roles, grants and decisions are about a principal, not about how it authenticated. Goal issue #64 (decision 2) removes the distinction.

Alternatives not taken:

- Keeping a principal kind (person, agent) in the identity port. The core would carry a field no rule reads, and an issuer that does not tell the two apart could not be used.
- A delegation model, with an agent acting for a person and both on the decision call. It is policy for the outside layer to hold, and the decision call can carry what that layer needs.

## Decision

- codehub has one kind of principal. It does not tell agents from people.
- How a token is obtained (device grant, token exchange, client credentials) belongs to the issuer and the client, not to codehub. codehub verifies the token it is given (ADR 0029).
- The agent-specific path of ADR 0006 and ADR 0021, and the plan that agents hold tokens obtained by Token Exchange at the authorisation server, are superseded. The rest of ADR 0006 and ADR 0021 stands.
- Runners remain a separate identity, decided with the runner design (`docs/architecture/runner.md`).

## Consequences

- The identity port returns a principal, identified by issuer and subject, with no kind.
- A principal's grants and decisions are the same however its token was obtained.
- ADR 0021's reason for not using Token Exchange for the CLI's person token no longer bears on codehub: the CLI's token sources are in ADR 0031.
- An outside layer that distinguishes agents does so in its own policy.

## Signs the decision was wrong

- A rule is needed that depends on whether the caller is an agent, such as a limit per agent, and the core cannot express it from the principal and the decision call.
- Audit must tell an agent's action from a person's and the token's claims cannot carry that to the core's log.
