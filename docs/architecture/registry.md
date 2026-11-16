# Registry

The container image registry: what holds the images, how a client is authorised, and how image names map to repositories.

## Current decisions

- **CNCF Distribution v3 holds the images** on S3-compatible storage, in a bucket or prefix of its own, separate from the source objects (ADR 0053). It runs beside codehub from the upstream image, and its replicas share one `http.secret`.
- **codehub is the token service** (ADR 0053):
  - The `api` role serves `GET /v1/registry/token` (`service`, repeated `scope`).
  - A caller authenticates with a bearer token, or with HTTP Basic whose password is the access token. The token is verified as any API call's is.
  - The answer is an ES256 JWT for the configured issuer and service, valid for 300 seconds. Its `access` claim lists only the actions allowed.
  - A missing or invalid credential is `401`. A foreign `service` is `400`. The endpoint is `404` unless the issuer, the service name and the signing key path are all configured.
- **An image is named `<org>/<repo>/<name>`**, with `<name>` one or more path components (ADR 0053).
  - `pull` is the action `image_pull` and `push` is `image_push`, asked on the repository with no path, in one batched decision per request.
  - Unknown names, other resource types and other actions are granted nothing, answered the same as a denial.
- **Distribution trusts codehub's public key** through its `jwks` option with `signingalgorithms: [ES256]` (ADR 0053).

## Rationale

- One access model: the grants and the decision point that govern source govern images (ADR 0053).
- Distribution is a maintained implementation of the OCI Distribution API, conformance-tested, and stateless apart from its storage, so it scales by replicas (ADR 0053).
- A separate bucket keeps the registry's garbage collection out of codehub's objects (ADR 0053).

## Open questions

- **Image deletion and garbage collection.** Options: Distribution's offline garbage collection on a schedule; a `delete` action mapped to a new grant. Recommendation: wait until a deployment's bucket growth asks for it.
- **Image events.** Distribution's notifications could start checks on a push. Recommendation: not until a repository asks for an image-triggered check.
