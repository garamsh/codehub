# Registry

The container image registry: what holds the images, how a client is authorised, and how image names map to repositories.

## Current decisions

- **CNCF Distribution v3 holds the images** on S3-compatible storage. It uses a bucket or prefix of its own, separate from the source objects, and runs beside codehub from the upstream image (ADR 0053).
- **codehub is the registry's token service**, under the Distribution token authentication specification (ADR 0053). Its token is the authorisation decision's answer, signed for the registry. It is never accepted by codehub itself, so codehub stays a resource server (`access.md`).
- **The token endpoint** is `GET /v1/registry/token`, in the user API. `interfaces.md` records its contract.
- **Image names are `<org>/<repo>/<name>`.** A push or pull is the action `image_push` or `image_pull` on that repository, decided as any action is (`access.md`). Whatever cannot be decided, such as an unknown name, the catalogue or deletion, is granted nothing, and is answered as a denial.
- **A push needs both grants.** Distribution asks for `pull` as well as `push` of every push, so a person holding `image_push` and not `image_pull` gets a token granting `push` and cannot complete one; the check environment's registry stores blobs through itself (`storage.redirect.disable`), because its redirect would name the object store by its compose name.
- **The signing key** is ES256. Distribution trusts it through a JWKS file that the deployment derives from the key; Distribution v3.1.2 starts and answers its 401 challenge with `auth.token.jwks` and `signingalgorithms: [ES256]` set and no `rootcertbundle`, so no certificate is derived (ADR 0053's open question, settled on the running registry in the pull request of #284). A key is rotated by trusting both keys first, then switching the signer (ADR 0053).
- **The token endpoint is off unless it is configured.** `CODEHUB_REGISTRY_ISSUER` (the token's `iss`, the registry's `auth.token.issuer`), `CODEHUB_REGISTRY_SERVICE` (its `aud`, `auth.token.service`), `CODEHUB_REGISTRY_KEY_FILE` (the PEM PKCS#8 P-256 key) and `CODEHUB_REGISTRY_KEY_ID` (the `kid` the JWKS names) are all given or none. With none the route answers 404, with some the server stops at start naming the missing one, and a key that is not an ES256 key stops it too. The role subcommands that serve the user API (`api`, `all`) read the settings, and `runner-api` does not.
- **The token** lasts 300 seconds and carries `access` for the scopes asked, at most 16 of them: the actions `pull` and `push` the caller is allowed, and none for a scope that is malformed, names an unknown repository or asks for anything else. The decision port is asked at most twice per request, once for `image_pull` and once for `image_push`, over every repository named. A request for another service is a 400. The credential is a bearer token or Basic credentials whose password is a codehub token; the 401 challenges with `Basic realm="codehub"`, and a method other than GET is a 405 with `Allow: GET`, which sends the Docker client's OAuth2 `POST` back to `GET`.
- **The check environment** runs the registry as the compose profile `registry` (`make e2e`): Distribution v3.1.2 pinned by digest, storing in the bucket `CODEHUB_TEST_REGISTRY_BUCKET` of the check object store, configured by `dev/registry/config.yml` and `REGISTRY_*` variables, trusting the JWKS the e2e example `garam_keys` writes with the signing key.

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
