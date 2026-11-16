# ADR 0053: Container images are held by CNCF Distribution on the object store, and codehub issues its registry tokens

> Status: accepted
> Date: 2026-11-16

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

Users build container images in their repositories, and runs and checks need somewhere to push and pull them (goal #273, decision 4). Each organisation already has members, grants and a decision point (ADR 0021, ADR 0030). A registry that kept its own users and permissions would be a second access model to keep in step with the first.

The OCI Distribution specification defines how a client pushes and pulls. The Distribution token authentication specification lets a registry delegate access decisions: the registry answers an unauthenticated request with a challenge naming a token service, the client gets a short-lived token for the scopes it needs, and the registry checks the token's signature and the actions it lists. CNCF Distribution implements the registry side. It stores blobs and manifests on S3-compatible storage, and it keeps no state of its own besides that storage, so replicas scale behind a load balancer.

## Decision

1. **The registry is CNCF Distribution (v3), with its S3 storage driver.**
   - It is run beside codehub as its own process, from the upstream image, and codehub does not embed or fork it.
   - It stores into a bucket, or a prefix of one, that the operator configures. This is not codehub's object-store bucket for source objects, so a registry's garbage collection never walks codehub's objects.
   - Replicas share one `http.secret`, so an upload begun on one replica can be finished on another.
2. **codehub issues the registry's tokens.**
   - The `api` role serves the token endpoint `GET /v1/registry/token`. Its query parameters are `service` and `scope`, as the token specification defines them; `scope` may repeat.
   - The caller authenticates in one of two ways: with `Authorization: Bearer <token>`, or with HTTP Basic, where the password is the codehub access token and the user name is ignored. Basic is what `docker login` and other OCI clients send.
   - The token is verified through the identity-verification port (ADR 0029), as any API call's token is.
   - A request with no credential, or with one that fails verification, is answered `401` with no token.
3. **The token** is a JWT signed with ES256 (ECDSA P-256 with SHA-256). Its claims are:
   - `iss`: the configured issuer;
   - `sub`: the caller's subject;
   - `aud`: the `service` the request named, which must equal the configured service name; another service is refused with `400`;
   - `exp`: issue time plus 300 seconds;
   - `nbf`, `iat` and a random `jti`;
   - `access`: the granted subset of each requested scope.

   The JOSE header carries a `kid`. The private key is read from a PEM file at a configured path. Distribution trusts the public key through its `jwks` option, with `signingalgorithms: [ES256]`.
4. **Image names are `<org>/<repo>/<name>`.**
   - `<name>` is one or more path components, as the OCI specification allows. The first two components name an existing organisation and repository.
   - A scope naming an unknown organisation or repository, or with fewer than three components, is granted no actions. It is answered like a denied scope, so a token request never reveals which repositories exist.
5. **Scopes map to two new actions on the repository.**
   - `pull` maps to `image_pull` and `push` maps to `image_push`. Each is asked of the decision port on the repository, with no path.
   - The token's `access` lists only the actions allowed. Docker's `pull,push` scope on a push is two decisions: one batched decision (ADR 0038) per request covers every scope it names.
   - Any other resource type or action, such as `registry:catalog:*` or `delete`, is granted nothing. A denied action is left out of `access` rather than failing the request: the token specification has the registry refuse the operation, not the token service.
6. **Grants gain `image_pull` and `image_push`.**
   - Like `run`, each is granted for the whole repository only, with the empty prefix. Neither is included by `read`, `write` or `run`, and neither includes them. An admin holds both.
   - The plain AuthZEN profile names them `image_pull` and `image_push`. The GRN profile names them `codehub:repo.image_pull` and `codehub:repo.image_push`. Whether garam's decision point allows them is garam's to settle, as for `codehub:repo.run`.
7. **Configuration.** The token endpoint is off unless all three of these are configured:
   - the issuer;
   - the service name;
   - the signing key path.

   With it off, `/v1/registry/token` answers `404`. Development and the tests run Distribution in `compose.yaml` against MinIO, with a key pair the dev tooling writes.

## Consequences

- An organisation's members, grants and decision point govern its images. No second user store exists.
- codehub stores nothing for images. Distribution and the bucket hold everything, so backup and restore of images are the bucket's.
- The registry scales by replicas of a stateless process. The token endpoint scales with the `api` role.
- codehub does not see pushes or pulls once a token is issued. An image event, such as a push triggering a check, would need Distribution's notifications, which this ADR does not configure.
- A token stays valid for its 300 seconds after a grant is revoked.
- Deleting images and garbage-collecting the bucket are not decided here.

## Alternatives not taken

- **Registry-native users (htpasswd) or a separate product (Harbor, Zot with its own users).** They give a second access model to keep in step with the grants.
- **Implementing the OCI Distribution API inside codehub.** Blob upload sessions, manifest validation and garbage collection would be rebuilt where a maintained, conformance-tested implementation exists.
- **Storing image blobs in codehub's own object store under its addresses.** OCI digests and codehub's addresses differ in form, and sharing the bucket ties two garbage collectors to one key space.
- **Long-lived tokens.** A revoked grant would keep working for as long as a token lives.

## Signs the decision was wrong

- Operators ask for image events (a push starting a check) often enough that the notification wiring becomes the main work.
- The 300-second token makes long pushes of large layers fail, which shows up as `401` mid-push.
- Organisations need per-image permissions below the repository, which a repository-level action cannot express.
- Distribution's S3 driver becomes the registry's bottleneck at a scale where a registry with its own metadata store would not be.
