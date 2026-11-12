# ADR 0042: The AuthZEN adapter may authenticate by private_key_jwt, with one client and one token per organisation

> Status: accepted; the bullet failing every refused token request as a server error superseded for `unauthorized_client` by ADR-0043
> Date: 2026-11-13

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0030 has the AuthZEN adapter authenticate to the decision point with "a configured bearer token obtained by OAuth client credentials". The server authenticates that request with a client id and a client secret (`CODEHUB_AUTHZEN_CLIENT_ID`, `CODEHUB_AUTHZEN_CLIENT_SECRET` in `crates/codehub/src/settings.rs`), and the adapter holds the one token for every request.

garam is the decision point codehub joins under garamsh/aurora#749. Its PM answered how it admits codehub on garamsh/garam#1760 (comment 6830113892):
- codehub's principal in an organisation is `grn:<org>/iam/service/codehub`;
- codehub holds one private key, and garam holds its public JWK;
- for a request about organisation `<org>`, codehub presents that organisation's token. It obtains it by client credentials, authenticated with an RFC 7523 `private_key_jwt` assertion whose `client_id`, `iss` and `sub` are that principal, asking `audience` `policy`;
- a token lives at most 300 seconds;
- there is no client secret and no token unbound to an organisation.

garam's reason: a token bound to one organisation turns a wrong resource chosen by codehub into a refusal rather than a question answered across organisations, and its audit names an organisation for every denial.

Every resource the authorisation decision port names carries its organisation: an organisation, or a repository or path in one. The adapter can therefore choose a token from the resource alone. No caller today asks about resources of two organisations in one call: `require_paths` in `crates/app/src/use_case/access.rs` takes one organisation.

Alternatives not taken:

- **Ask garam for one client serving every organisation.** It is the unbound caller garam refuses, for the reason above.
- **A client secret per organisation.** It means one secret per organisation to provision and rotate, against one key for all of them. garam offers no secret either.
- **The token endpoint as the assertion's audience**, which RFC 7523 allowed when published. RFC 10070 forbids it for client authentication "to address a security vulnerability identified in the previous requirements for those audience values" (its abstract).
- **Replace the client-secret mode.** The development stack, where Cerbos decides and the token comes from Keycloak's token endpoint, and any other AuthZEN deployment may still take a secret. The two modes differ only in how the adapter obtains a token.

## Decision

- The AuthZEN adapter's client authentication is selected by configuration: client secret (the ADR 0030 mode and the default), or `private_key_jwt` (RFC 7523 §2.2, as OpenID Connect Core 1.0 §9 profiles it).
- In `private_key_jwt` mode, as RFC 7523 reads after its update by RFC 10070 (November 2026). Where OpenID Connect Core 1.0 §9 recommends the token endpoint as the audience, RFC 10070 overrides it:
  - the client id setting holds a template with the organisation's name, such as `grn:{organisation}/iam/service/codehub`. The adapter fills it with the organisation of the resources it is deciding, and sends it as the `client_id` parameter;
  - the settings this mode requires are the template, the key file and the issuer identifier; the client secret is not used. A missing one fails at start;
  - the adapter signs the assertion with a private key read from a file path. The key never passes through a setting's value, an argument or a log line. The algorithm follows the key: ES256 for a P-256 key, RS256 for an RSA key;
  - the assertion's `iss` and `sub` are the filled client id. Its `aud` is the authorization server's issuer identifier (RFC 8414), configured beside the token endpoint, as its sole value; the token endpoint is never the audience. Its header carries `typ` `client-authentication+jwt`, as RFC 10070 recommends, and a `kid` where one is configured. It carries a fresh `jti`, and lives at most 60 seconds;
  - an optional configured audience is sent as the token request's `audience` parameter. No RFC defines that parameter for client credentials; garam asks for it (`policy`);
  - the adapter keeps at most one token per organisation. It renews a token before it expires, and once when the decision point rejects it. Each time it stores a token it drops every expired one, so an organisation no longer served holds nothing for longer than one token's life after the next request;
  - a token endpoint that refuses or fails to issue an organisation's token fails the request as a server error and allows nothing (ADR 0033). Nothing is cached for that organisation.
- A port call whose resources span organisations is split by organisation. Each group is decided with that organisation's token, as ADR 0041 decides one call, and the groups are sent one after another. The answers keep the order of the call.
- In client-secret mode one token serves every organisation, as ADR 0030 decided.
- In `private_key_jwt` mode this supersedes ADR 0030's "a configured bearer token": the client-credentials grant stays, and the one token becomes one per organisation. The rest of ADR 0030 stands.

## Consequences

- With garam as the decision point, codehub holds no secret shared with it: the key stays on the server, and garam holds only its public part.
- Token requests grow with the organisations served: at most one per organisation each token lifetime, plus one after a rejection.
- An organisation's first request after its token expires waits on one token request.
- A key file the server cannot read, or a key of another type, fails at start, before the server listens.
- ADR 0029 is unchanged. With garam the server verifies tokens offline as JWTs, so introspection needs no `private_key_jwt` client. A deployment that needs introspection against garam is a later decision.
- Whether garam issues a token for an organisation codehub has not yet created, which lazy `CreateOrganisation` (ADR 0030) needs, is garam's ADR-0149 to state. Until it does, such a request fails as a server error.
- What codehub's subject, actions and resources map to in garam, how directory changes reach it, and the runner as a garam principal are garam's ADR-0149, still being written. They may change other parts of ADR 0030, chiefly at-most-once delivery of directory changes.

## Signs the decision was wrong

- garam, or another decision point codehub serves, binds tokens to something other than the organisation: a repository, or the caller's own principal.
- The token request rate per organisation shows on the issuer's side.
- An operator needs a different key per organisation.
