# ADR 0053: Container images are held by CNCF Distribution on the object store, and codehub issues its registry tokens

> Status: accepted
> Date: 2026-11-16

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

Users build container images in their repositories. Runs and checks need somewhere to push and pull them (goal #273, decision 4). Each organisation already has members, grants and a decision point (ADR 0021, ADR 0030). A registry that kept its own users and permissions would be a second access model to keep in step with the first.

Three outside references bear on this, read on 2026-11-16:
- **The OCI Distribution Specification v1.1** defines how a client pushes and pulls, and the grammar of a repository name.
- **The Distribution token authentication specification and its JWT profile** (distribution.github.io/distribution/spec/auth/token/ and …/jwt/) let a registry delegate access decisions:
  1. The registry answers an unauthenticated request with a challenge that names a token service (`realm`) and a `service`.
  2. The client sends `GET` to the realm with `service` and zero or more `scope` parameters of the form `repository:<name>:<actions>`, authenticating with HTTP Basic or a bearer token.
  3. The client receives a JSON body with `token` (or `access_token`), `expires_in` and `issued_at`.
  4. The registry checks the JWT's signature, `iss`, `aud` and expiry, and checks that its `access` claim lists the action asked. The token service grants the intersection of what was asked and what is allowed, and a denied action is left out rather than failing the request.
- **CNCF Distribution v3.0** (configuration reference, `auth.token`) implements the registry side. It takes `realm`, `service` and `issuer`, trusts keys from `rootcertbundle` and, per the reference, from a `jwks` file, and limits algorithms with `signingalgorithms`. It stores blobs and manifests through its S3 driver and keeps no other state, so replicas scale behind a load balancer.

These alternatives were considered and not taken:
- **Registry-native users (htpasswd), or a separate product with its own users (Harbor, Zot).** Either is a second access model.
- **The OCI Distribution API implemented inside codehub.** Upload sessions, manifest validation and garbage collection would be rebuilt where a maintained, conformance-tested implementation exists.
- **Image blobs in codehub's own object store, under its addresses.** OCI digests and codehub's addresses differ in form, and sharing a key space ties two garbage collectors to it.
- **Tokens that live longer.** A revoked grant would keep working for the token's whole life.

## Decision

1. **The registry is CNCF Distribution v3, with its S3 storage driver.**
   - It runs beside codehub, as its own process, from the upstream image. codehub does not embed or fork it.
   - It stores into a bucket, or a prefix of one, that the operator configures. That bucket is not codehub's bucket for source objects.
   - Its replicas share one `http.secret`, so an upload begun on one replica can be finished on another.
   - Its `auth.token` points `realm` at codehub's token endpoint.
2. **codehub issues the registry's tokens.** This narrows ADR 0006's "the core issues no tokens" in one respect, and only that one.
   - The token is not an identity credential. It is the authorisation decision's answer, signed, for a third component that cannot call the decision port.
   - The token never authenticates a caller to codehub. codehub stays a resource server for every token it accepts.
3. **The token endpoint is `GET /v1/registry/token`.** It is served by the process that serves the user API, which is the user API's role once the server is split into roles.
   - **Credentials.** The caller presents either `Authorization: Bearer <access token>` or HTTP Basic, whose password is the access token and whose user name is ignored. Basic is what `docker login` and other OCI clients send.
     - This sets aside ADR 0024's "callers present bearer tokens" for this route only.
     - The credential is verified through the identity-verification port (ADR 0029), as any API call's is.
     - A missing or invalid credential is answered `401`, with `WWW-Authenticate: Basic realm="codehub"`.
   - **`service`** must be present and equal the configured service name. A missing or different value is `400`.
   - **`scope`.** It may be absent, and then the token grants nothing. Absent is what a client sends on `docker login` or on a registry ping. It may also repeat, at most 16 times; more is `400`.
   - **Other parameters.** `offline_token`, `client_id` and `account` are ignored, and no refresh token is issued. `POST` to the endpoint, the OAuth2 flow, is answered `405` with `Allow: GET`. The Docker client falls back to `GET` with Basic when the `POST` fails. Other clients are verified by the implementation's tests.
   - **Response.** A success is `200` with `{"token": …, "access_token": …, "expires_in": …, "issued_at": …}`, both token fields holding the same JWT.
   - **Errors.** An error is an RFC 9457 problem detail, as ADR 0024 sets for every route. The token specification leaves the token endpoint's error body to the service.
   - The route is in the OpenAPI document.
4. **The token is a JWT signed with ES256.**
   - Its header carries `typ: JWT`, `alg: ES256` and the configured `kid`.
   - Its claims:
     - `iss`: the configured issuer;
     - `sub`: the caller's subject;
     - `aud`: the service;
     - `iat` and `nbf`: now;
     - `exp`: now plus 300 seconds. It is not capped at the presented token's expiry: the identity verification port reports no expiry (ADR 0029), and the token outlives the presented one by at most 300 seconds;
     - `jti`: a random value;
     - `access`: one entry per scope, listing only the actions allowed.
   - `expires_in` is the seconds from `iat` to `exp`.
5. **Image names are `<org>/<repo>/<name>`.**
   - A scope's name must match the OCI Distribution v1.1 repository-name grammar and have at least three components. The first two must name an existing organisation and repository. A codehub name may end in `-`, which the OCI grammar does not allow. A repository whose organisation or repository name does not fit the grammar therefore has no image names, and scopes for it are malformed.
   - Granted entries echo the requested name exactly.
   - A scope that is malformed, or that names an unknown organisation or repository, is granted no actions and answered the same as a denied one. So the response does not say which repositories exist. Its timing may still differ, because a known repository costs a decision call.
6. **Scopes map to two actions on the repository.**
   - `pull` is `image_pull` and `push` is `image_push`. Each is asked of the authorisation decision port on the repository, with no path.
   - The decision port answers one action over many resources, so a request makes at most two port calls: one for `image_pull` over every repository its scopes name, one for `image_push` over every repository its scopes name. Each port call is one call to the decision point where batching is on (ADR 0038, ADR 0041), and one call per repository where it is off, so at most 32 calls for the 16 scopes.
   - Any other resource type or action, such as `registry:catalog:*`, `delete` or `*`, is granted nothing.
7. **Grants gain `image_pull` and `image_push`.**
   - Like `run`, each is granted for the whole repository only. Neither is included by `read`, `write` or `run`, and neither includes them. An admin holds both.
   - The plain AuthZEN profile names them `image_pull` and `image_push`, with no path property. The GRN profile names them `codehub:repo.image_pull` and `codehub:repo.image_push`. Whether garam's decision point allows them is garam's to settle, as for `codehub:repo.run`.
8. **Signing is a driven port, Registry token issuance.**
   - Given the subject, the service, the granted entries in the application's terms, the issue time and an expiry bound, it returns an opaque signed token and its lifetime. No claim set appears in its signature.
   - Its production adapter signs the JWT of point 4 with an ES256 private key read from a PEM file.
   - Its second adapter, for the harness, returns a token the harness can read back.
9. **Configuration and keys.**
   - The endpoint is off, answering `404`, unless the issuer, the service name, the signing key path and the key id are all configured.
   - Distribution trusts the matching public key from a JWKS file, with `signingalgorithms: [ES256]`.
   - **Producing the JWKS:** whoever deploys the pair produces the JWKS from the key: the dev tooling for compose, the Helm chart for Kubernetes.
   - **Rotating a key:**
     1. Add the new public key to the JWKS beside the old one, and restart Distribution.
     2. Switch codehub to the new key and id.
     3. After 300 seconds, remove the old key from the JWKS, and restart Distribution again.
   - Development and the tests run Distribution in `compose.yaml` against MinIO.

## Consequences

- An organisation's members, grants and decision point govern its images. No second user store exists.
- codehub stores nothing for images. Distribution and its bucket hold all of it, so backing up and restoring images is the bucket's job.
- The registry scales by replicas of a stateless process. The token endpoint scales with the process that serves the API, at two port calls per token: two decision-point calls with batching on, and one per named repository and action without it.
- **A holder of `image_push` can overwrite any tag of the repository's images,** including images that runs and checks pull. This is the same trust that `write` gives over the code those runs build.
- **`docker login` stores the user's codehub access token as the registry password** in the client's credential store. That token is not limited to the registry, and it expires on the issuer's schedule, so a stored login stops working when the token does. A credential helper that fetches a fresh token is the client's choice.
- **A token stays valid for up to 300 seconds after a grant is revoked.**
- **codehub does not see a push or a pull once it has issued the token.** An image event, such as a push starting a check, needs Distribution's notifications, which this ADR does not configure.
- **Not decided here:** deleting images, garbage-collecting the bucket, and what happens to a deleted repository's images if its name is reused. Repository deletion is itself reserved (`tenancy.md`).

## Signs the decision was wrong

- Operators ask for image events, such as a push starting a check, often enough that the notification wiring becomes the main work.
- Clients fail on the 300-second token: a manifest put after a long layer upload is refused as expired.
- Organisations need permissions per image, below the repository, which a repository-level action cannot express.
- Distribution's S3 driver becomes the bottleneck at a scale where a registry with its own metadata store would not be.
