# Access

Who is calling the system and whether the call is allowed: the ports the core uses for identity, authorisation and provisioning, and the standards their default adapters speak.

## Current decisions

- **The core holds no user directory, issues no tokens and evaluates no policy** (ADR 0006). It verifies who is calling, asks whether the call is allowed, and receives provisioning.
- **Identity and policy are external services.** A later service operated by the project owner replaces the default adapters without changing the application.
- **The core is an OAuth resource server.** It validates access tokens issued elsewhere; it is not an authorisation server.
- **The core is the policy enforcement point.** The policy decision point is outside the core, as are the policy engine and its data. The core defines what it asks.
- **There is one kind of principal** (ADR 0027), and runners are principals too (ADR 0034). The core does not tell agents from people, and how a token is obtained (device grant, token exchange, client credentials) is the issuer's and the client's.
- **An outside layer joins through standards only** (ADR 0028). The core accepts only tokens bound to its one configured audience, shares organisation names with the outside layer with no mapping table, and resolves scope through the decision call.
- **Identity is verified as an OIDC JWT offline or by RFC 7662 introspection**, one selected for the whole deployment by configuration (ADR 0029). In both modes the core only rejects; token lifetimes stay the issuer's.
- **Authorisation is the PostgreSQL default or AuthZEN**, selected by configuration (ADR 0030). Selecting AuthZEN is the external mode: the core keeps no members or grants, the member endpoints are refused, creating an organisation writes no admin grant, and an organisation is created on first use when `CreateOrganisation` is allowed. An unreachable or failing decision point fails the request as a server error and allows nothing, with no caching (ADR 0033).

### Ports

Declared in the application's terms (`docs/architecture/structure.md`), named for the conversation. No token, certificate, claim set or SCIM resource appears in a signature.

| Port | Direction | Conversation | Default adapter speaks |
|---|---|---|---|
| Identity verification | Driven by the application | Given a credential presented with a request, return who the caller is, a principal identified by issuer and subject, or reject it | Two adapters, one selected by configuration (ADR 0029): an OpenID Connect Core 1.0 JWT bearer access token verified offline as a resource server, or RFC 7662 token introspection, where the core authenticates to the endpoint with client credentials and rejects a response without `active: true` or without its audience |
| Authorisation decision | Driven by the application | Given subject, action, resource and context, return allow or deny; the resource carries the path (ADR 0021) | Default adapter: grants evaluated in PostgreSQL. Second adapter, selected by configuration (ADR 0030): OpenID AuthZEN Authorization API 1.0, Access Evaluation, and Access Evaluations where the operator turns it on and the decision point advertises it (ADR 0038, ADR 0041), over HTTP JSON, authenticated with a bearer token obtained by OAuth client credentials, with a client secret or, where configured, a `private_key_jwt` assertion and one client and token per organisation (ADR 0042); Subject, Action and Resource are mapped in the application's terms and the path is a resource property |
| Membership | Driven by the application | Add and remove organisation members and their grants (ADR 0021); the default authorisation adapter reads them in PostgreSQL | PostgreSQL tables, the same ones the default authorisation adapter evaluates; in external mode the member endpoints are refused and the port is not used |
| Directory change | Driven by the application | Report organisation and repository creation and deletion, so an outside layer can learn of them (ADR 0030) | An in-process adapter and a no-op; delivery is at most once; deletion events are reserved and not emitted until deletion exists |
| Provisioning intake | Driving | Receive creation, update and deactivation of users and groups from a directory | SCIM 2.0 server (RFC 7643, RFC 7644) |

- A runner is a principal and is verified by the identity verification port like any caller. The runner identity verification port with mutual TLS was dropped by ADR 0034; which principals take jobs is the runner registry's (`runner.md`).
- Per `structure.md`, each port has a second adapter for a test harness: a fixed set of identities and a decision table. A port whose second adapter proves unnecessary is dropped.
- `docs/architecture/runner.md` records runner registration and the credentials a runner is given. `docs/architecture/interfaces.md` records the `ch` CLI and its token sources, which present the tokens this document validates. `docs/architecture/tenancy.md` and `docs/architecture/source-control.md` define the organisations, repositories and resources that subject, action and resource name. This document records none of those.

### Roles and grants

- **Roles are `admin` and `member`** (ADR 0021). Any authenticated principal may create an organisation and becomes its admin; an admin may take every action on the organisation and its repositories, administers members and grants, and administers runners (ADR 0034).
- **A member holds grants** of (repository, path prefix, `read` or `write`). `write` includes `read`; a prefix matches by whole components; the empty prefix is the whole repository.
- **The core sends the path as a resource attribute.** `land` asks for `write` on every path the draft changes and rejects the whole landing, naming every denied path; `read` and `log` ask for `read` on their path.
- **Decisions on many paths are asked in one call** (ADR 0038). The AuthZEN adapter sends one Access Evaluations request only where `CODEHUB_AUTHZEN_EVALUATIONS` is on, off by default, and the decision point advertises `access_evaluations_endpoint`; otherwise it sends one Access Evaluation per path. The other adapters decide each path (ADR 0041).
- **The AuthZEN adapter's token may be bound to an organisation** (ADR 0042). With `private_key_jwt` client authentication the client id is a template filled with the organisation of the resources decided, the adapter keeps one token per organisation, and a call spanning organisations is decided per organisation in the call's order. With a client secret one token serves every organisation.
- **The default adapter keeps memberships and grants in PostgreSQL** and evaluates them there. With the AuthZEN adapter (external mode) the core keeps none and the decision point decides.

## Rationale

- Separation from the core and the choice of standards: ADR 0006; its agent and Token Exchange parts are superseded by ADR 0027.
- Ports are what let an adapter be replaced: ADR 0001.
- Roles, grants, the path as a resource attribute, the default adapter and the CLI's Device Authorization Grant, and the alternatives not taken: ADR 0021; its agent and Token Exchange parts are superseded by ADR 0027.
- One kind of principal, and why a token's origin is not the core's concern: ADR 0027.
- One audience, shared organisation names and no claim mapping: ADR 0028.
- JWT or introspection, and the rules for an introspection response: ADR 0029.
- The AuthZEN adapter, external mode, fail-closed on failure and the directory-change port: ADR 0030; a failure answered as a server error rather than a denial: ADR 0033.
- One decision call for many paths, and the fallback where the batch endpoint is not offered: ADR 0038. The batch only on the operator's setting, because Cerbos 0.57.0 answers every evaluation sharing a resource id with the first one's decision: ADR 0041.
- Client authentication by `private_key_jwt` and a token per organisation, because garam binds codehub's principal to one organisation and offers no client secret: ADR 0042.
- The CLI's token sources and RFC 8693 exchange: ADR 0031.
- Runners as registered principals, with no runner credential and no mutual TLS: ADR 0034.

Status of each standard, checked on 2026-11-11 against its specification page, and RFC 7523 on 2026-11-13 against its RFC Editor record:

| Standard | Status |
|---|---|
| OpenID Connect Core 1.0 incorporating errata set 2 | Final, 15 December 2023 |
| The OAuth 2.1 Authorization Framework | Internet-Draft draft-ietf-oauth-v2-1-17, not an RFC; datatracker dates it 2026-10-30 |
| RFC 8693, OAuth 2.0 Token Exchange | Proposed Standard |
| RFC 7662, OAuth 2.0 Token Introspection | Proposed Standard, October 2015 |
| OpenID Authorization API 1.0 (AuthZEN) | Final, 11 January 2026 |
| RFC 7523, JWT Profile for OAuth 2.0 Client Authentication and Authorization Grants | Proposed Standard, May 2015; updated by RFC 10070 (Proposed Standard, November 2026), which makes the issuer identifier the sole audience of a client authentication assertion |
| RFC 7643 and RFC 7644, SCIM Core Schema and Protocol | Proposed Standards, September 2015; both updated by RFC 9865 (cursor pagination) and RFC 9967 (SCIM profile for Security Event Tokens) |

## Open questions

- **Policy decision point behind the AuthZEN adapter.** The default adapter evaluates grants in PostgreSQL (ADR 0021) and the AuthZEN adapter exists (ADR 0030). Development and the tests use Cerbos (`compose.yaml`, policies in `dev/cerbos/policies/`); what a production deployment reaches is open. Options: OpenFGA, Cedar, Cerbos, another engine reachable through AuthZEN, or a small engine written for the project. Recommendation: require that the engine match path prefixes and be reachable through AuthZEN.
- **Filtering directory listings by path permission.** A listing of a directory may include entries the caller holds no `read` grant for. Options: omit them; show them and refuse the read; ask the decision port per entry. No recommendation yet.
- **Identity provider for a single-machine deployment.** Development and the tests run existing self-hosted providers beside the core: Dex for the device grant and Keycloak for introspection and token exchange (`compose.yaml`). Which provider a single-machine deployment runs is open. Recommendation: an existing self-hosted provider, because a provider inside the core contradicts the separation.
- **SCIM scope.** Which resources are served (Users, Groups, both) and how provisioned groups reach the policy engine (through the core, or by the engine reading the directory directly). Recommendation: Users and Groups, with the engine reading the directory the external service owns, so the core stores only the identifiers it needs for audit.
- **Which OAuth 2.1 draft is the reference** while it is not an RFC. The datatracker shows -17 on the date above. Recommendation: take validation rules from the RFCs the draft consolidates, and record a draft number only where a rule exists in one draft and not another.
- **Introspection per action.** ADR 0029 selects JWT or introspection for the whole deployment. Options: keep it global; choose per action. Recommendation: keep it global until a deployment needs a revoked token refused at once for some actions only.
- **Reconciling directory changes.** Delivery is at most once (ADR 0030), so an outside layer can miss a creation or deletion. Options: a periodic listing the outside layer reads; a durable queue behind the port. Recommendation: decide with the first adapter that reports to a real outside layer.
- **Validating a name created lazily.** In external mode an organisation is created on first use under the name in the request (ADR 0030). Options: accept any name the rules for organisation names allow; ask the decision point whether the name exists on its side. Recommendation: accept any valid name, since the decision point already allowed `CreateOrganisation`.
- **Deleting an organisation in external mode**, and what the decision point is asked for it. No option examined.
