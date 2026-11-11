# Access

Who is calling the system and whether the call is allowed: the ports the core uses for identity, authorisation and provisioning, and the standards their default adapters speak.

## Current decisions

- **The core holds no user directory, issues no tokens and evaluates no policy** (ADR 0006). It verifies who is calling, asks whether the call is allowed, and receives provisioning.
- **Identity and policy are external services.** A later service operated by the project owner replaces the default adapters without changing the application.
- **The core is an OAuth resource server.** It validates access tokens issued elsewhere; it is not an authorisation server.
- **The core is the policy enforcement point.** The policy decision point is outside the core, as are the policy engine and its data. The core defines what it asks.

### Ports

Declared in the application's terms (`docs/architecture/structure.md`), named for the conversation. No token, certificate, claim set or SCIM resource appears in a signature.

| Port | Direction | Conversation | Default adapter speaks |
|---|---|---|---|
| Identity verification | Driven by the application | Given a credential presented with a request, return who the caller is (a person, or an agent or the CLI acting for a person) and the organisation or repository it is scoped to, or reject it | OpenID Connect Core 1.0 and OAuth 2.1 bearer access tokens, validated as a resource server; a person's CLI obtains its token with the OAuth 2.0 Device Authorization Grant (RFC 8628); agents hold tokens obtained by OAuth 2.0 Token Exchange (RFC 8693) at the authorisation server, not yet implemented (ADR 0021) |
| Runner identity verification | Driven by the application | Given a runner's transport-level credential, return which runner it is | Mutual TLS client certificates |
| Authorisation decision | Driven by the application | Given subject, action, resource and context, return allow or deny; the resource carries the path (ADR 0021) | Default adapter: grants evaluated in PostgreSQL. Later adapter: OpenID AuthZEN Authorization API 1.0, Access Evaluation, over HTTPS JSON |
| Membership | Driven by the application | Add and remove organisation members and their grants, and read them (ADR 0021) | PostgreSQL tables, the same ones the default authorisation adapter evaluates |
| Provisioning intake | Driving | Receive creation, update and deactivation of users and groups from a directory | SCIM 2.0 server (RFC 7643, RFC 7644) |

- Identity verification and runner identity verification are two ports because the credential, its trust root and its lifecycle differ.
- Per `structure.md`, each port has a second adapter for a test harness: a fixed set of identities and a decision table. A port whose second adapter proves unnecessary is dropped.
- `docs/architecture/runner.md` records runner registration and the credentials a runner is given. `docs/architecture/interfaces.md` records the `ch` CLI and the MCP server, which present the tokens this document validates. `docs/architecture/tenancy.md` and `docs/architecture/source-control.md` define the organisations, repositories and resources that subject, action and resource name. This document records none of those.

### Roles and grants

- **Roles are `admin` and `member`** (ADR 0021). Any authenticated principal may create an organisation and becomes its admin; an admin may take every action on the organisation and its repositories and administers members and grants.
- **A member holds grants** of (repository, path prefix, `read` or `write`). `write` includes `read`; a prefix matches by whole components; the empty prefix is the whole repository.
- **The core sends the path as a resource attribute.** `land` asks for `write` on every path the draft changes and rejects the whole landing, naming every denied path; `read` and `log` ask for `read` on their path.
- **The default adapter keeps memberships and grants in PostgreSQL** and evaluates them there; the AuthZEN adapter replaces it later.

## Rationale

- Separation from the core and the choice of standards: ADR 0006.
- Ports are what let an adapter be replaced: ADR 0001.
- Roles, grants, the path as a resource attribute, the default adapter and the CLI's Device Authorization Grant, and the alternatives not taken: ADR 0021.

Status of each standard, checked on 2026-11-11 against its specification page:

| Standard | Status |
|---|---|
| OpenID Connect Core 1.0 incorporating errata set 2 | Final, 15 December 2023 |
| The OAuth 2.1 Authorization Framework | Internet-Draft draft-ietf-oauth-v2-1-17, not an RFC; datatracker dates it 2026-10-30 |
| RFC 8693, OAuth 2.0 Token Exchange | Proposed Standard |
| OpenID Authorization API 1.0 (AuthZEN) | Final, 11 January 2026 |
| RFC 7643 and RFC 7644, SCIM Core Schema and Protocol | Proposed Standards, September 2015; both updated by RFC 9865 (cursor pagination) and RFC 9967 (SCIM profile for Security Event Tokens) |
| RFC 8705, OAuth 2.0 Mutual-TLS Client Authentication and Certificate-Bound Access Tokens | Proposed Standard |
| MCP authorization, revision 2026-07-28 | The MCP server is an OAuth 2.1 resource server (cites draft-ietf-oauth-v2-1-13) and must implement OAuth 2.0 Protected Resource Metadata (RFC 9728) |

## Open questions

- **Policy decision point behind the AuthZEN adapter.** The default adapter evaluates grants in PostgreSQL (ADR 0021). Options for what the AuthZEN adapter later reaches: OpenFGA, Cedar, another engine reachable through AuthZEN, or a small engine written for the project. Recommendation: require that the engine match path prefixes and be reachable through AuthZEN.
- **Filtering directory listings by path permission.** A listing of a directory may include entries the caller holds no `read` grant for. Options: omit them; show them and refuse the read; ask the decision port per entry. No recommendation yet.
- **Identity provider for the single-machine stage.** Options: an existing self-hosted OpenID provider run beside the core; a minimal development-only provider shipped as a separate program, not part of the core; static tokens through the test adapter. Recommendation: an existing self-hosted provider, because a provider inside the core contradicts the separation and a development-only one is code to keep.
- **Token lifetimes** (access token, refresh token, delegated agent token from token exchange, runner credential). Options: short access tokens with refresh for people; per-job lifetimes for agents. Recommendation: decide with the identity provider, since the issuer sets them; the core only rejects expired tokens.
- **Token audience and scope.** Whether the audience is one per core, per organisation or per repository, and how organisation and repository scope is carried (scopes, claims, or resolved by the decision call). Recommendation: one audience for the core, identified by a resource indicator (RFC 8707, which the MCP authorization revision above requires), with scope resolved by the decision call.
- **Behaviour when the decision service is unreachable.** Options: deny; allow a cached decision for a bounded time; allow. Recommendation: deny, with caching only where a revocation delay is accepted and recorded here.
- **Decision caching and batching** at the target scale of hundreds of concurrent agents. Options: none; per-request memo; batched evaluations if the engine supports them. Recommendation: measure before adding.
- **SCIM scope.** Which resources are served (Users, Groups, both) and how provisioned groups reach the policy engine (through the core, or by the engine reading the directory directly). Recommendation: Users and Groups, with the engine reading the directory the external service owns, so the core stores only the identifiers it needs for audit.
- **Runner identity.** Whether the certificate alone identifies a runner or a token bound to it (RFC 8705) is also used, and who issues runner certificates. Settled together with `docs/architecture/runner.md`.
- **Which OAuth 2.1 draft is the reference** while it is not an RFC. The MCP revision cites -13; the datatracker shows -17. Recommendation: take validation rules from the RFCs the draft consolidates, and record a draft number only where a rule exists in one draft and not another.
