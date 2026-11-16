# Registry

The container image registry: what holds the images, how a client is authorised, and how image names map to repositories.

## Current decisions

- **CNCF Distribution v3 holds the images** on S3-compatible storage. It uses a bucket or prefix of its own, separate from the source objects, and runs beside codehub from the upstream image (ADR 0053).
- **codehub is the registry's token service**, under the Distribution token authentication specification (ADR 0053). Its token is the decision point's answer, signed for the registry. It is never accepted by codehub itself, so codehub stays a resource server (`access.md`).
- **The token endpoint** is `GET /v1/registry/token`, in the user API. `interfaces.md` records its contract.
- **Image names are `<org>/<repo>/<name>`.** A push or pull is the action `image_push` or `image_pull` on that repository, decided as any action is (`access.md`). Whatever cannot be decided, such as an unknown name, the catalogue or deletion, is granted nothing, and is answered as a denial.
- **The signing key** is ES256. Distribution trusts it through a JWKS file that the deployment derives from the key. A key is rotated by trusting both keys first, then switching the signer (ADR 0053).

## Rationale

- One access model: the grants and the decision point that govern source govern images (ADR 0053).
- Distribution is a maintained, conformance-tested implementation of the OCI Distribution API. It keeps no state besides its storage, so it scales by replicas (ADR 0053).
- A separate bucket keeps the registry's garbage collection away from codehub's objects (ADR 0053).

## Open questions

- **Image deletion and garbage collection.** Options:
  - Distribution's offline garbage collection, run on a schedule;
  - a `delete` action mapped to a new grant.

  Recommendation: wait until a deployment's bucket growth asks for it.
- **Images of a deleted repository whose name is reused.** Repository deletion is reserved (`tenancy.md`). Recommendation: decide with deletion. Either delete the images with the repository, or key the bucket prefix by the repository's never-reused id.
- **Image events.** Distribution's notifications could start a check when an image is pushed. Recommendation: not until a repository asks for an image-triggered check.
