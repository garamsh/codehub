# ADR 0029: A token is verified as an OIDC JWT or by RFC 7662 introspection, selected by configuration

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0006 made the core an OAuth resource server that validates tokens issued elsewhere, and the default adapter validates an OpenID Connect JWT offline. Goal issue #64 (decision 4) adds a second way to verify, for deployments that need a revoked token refused at once: RFC 7662 introspection asks the issuer whether a token is active. RFC 7662 is a Proposed Standard of October 2015 (rfc-editor.org/info/rfc7662, read on 2026-11-11).

Alternatives not taken:

- Choosing between the two per action. It doubles the paths the core tests and secures, and is out of scope for M3.
- Introspection only. Every request would cost a call to the issuer even where a revocation delay is accepted.
- Caching introspection results. It reintroduces the revocation delay introspection exists to remove.

## Decision

- The existing OIDC JWT adapter stays as it is. A second adapter verifies by introspection. Configuration selects one of the two for the whole deployment.
- The introspection adapter authenticates to the endpoint with client credentials, as RFC 7662 §2.1 requires of the protected resource.
- A response without `active: true`, or without codehub's audience (ADR 0028) in `aud`, is a rejection.
- The principal is (`iss`, `sub`). Where the response has no `iss`, the issuer is the configured one.
- In both modes the core only rejects; token lifetimes stay the issuer's.

## Consequences

- Introspection mode adds a call to the issuer on every request, and the issuer's availability and latency become the core's. An unreachable endpoint rejects the request.
- JWT mode accepts a token until it expires, so revocation is as slow as the token's lifetime.
- The deployment carries client credentials for the introspection endpoint as a secret.
- Both adapters sit behind the one identity verification port, so the application does not change (ADR 0001).

## Signs the decision was wrong

- Deployments need JWT for most actions and introspection for a few, and ask for the choice per action.
- Introspection latency shows in request latency at the target of hundreds of concurrent agents.
- Issuers omit `iss` from responses so often that the configured fallback hides a wrong issuer.
