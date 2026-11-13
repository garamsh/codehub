# Deployment

How to run the codehub server and runner images: what each image holds, what it is configured with, what it needs, and what to probe.

This file is the contract between codehub and whoever declares its deployment. Its two settings tables are compared with `codehub --help` and `codehub-runner --help` by `crates/codehub/tests/deployment.rs` and `crates/codehub-runner/tests/deployment.rs`, so a setting added or removed in code fails the tests until this file says so.

## Contents
- Images
- Server settings
- Runner options
- What the server needs
- Health
- How the runner reaches the server

## Images

A tag `vX.Y.Z` of this repository publishes both images, tagged `X.Y.Z` and never `latest` (`.github/workflows/release.yml`). Each is built from the repository root with the Dockerfile named below.

| Image | Dockerfile | Entrypoint | Notes |
|---|---|---|---|
| `codehub/server:X.Y.Z` | `deploy/server.Dockerfile` | `/usr/local/bin/codehub` | Exposes port 8080. |
| `codehub/runner:X.Y.Z` | `deploy/runner.Dockerfile` | `/usr/local/bin/codehub-runner` | Holds `/bin/sh` and coreutils, which the host driver runs a job's command under. |

- Both run as user 10001, not root, and are based on `debian:trixie-slim` with CA certificates.
- Both carry `org.opencontainers.image.version` and `org.opencontainers.image.revision` labels.
- No address, URL or secret is set at build time. Every setting comes from the variables below at run.
- Arguments given to the container are the options below, after the entrypoint.
- A push that fails between the two images leaves only the server published. The repositories are tag-immutable, so a re-run cannot repair it and the repair is the next version.

## Server settings

Every setting is an option and a variable of the same name, the option's name upper-cased with the prefix `CODEHUB_`. A variable and the option both given: the option wins. A secret is marked `yes` below, and its value is hidden in the help output and in the server's configuration output.

The server listens on `127.0.0.1:8080` unless `CODEHUB_LISTEN` says otherwise, so in a container set it to an address that is reachable, such as `0.0.0.0:8080`. It stops on SIGTERM or SIGINT after finishing the requests in flight.

| Option | Variable | Secret | Purpose |
|---|---|---|---|
| `--listen` | `CODEHUB_LISTEN` | no | The address to listen on. |
| `--postgres-url` | `CODEHUB_POSTGRES_URL` | yes | The PostgreSQL database holding the metadata. |
| `--postgres-pool-size` | `CODEHUB_POSTGRES_POOL_SIZE` | no | Connections each server process holds; the sum over processes stays below PostgreSQL's `max_connections` (ADR 0037). |
| `--landing-attempts` | `CODEHUB_LANDING_ATTEMPTS` | no | How often a landing is tried while `main` moves under it (ADR 0036). |
| `--runner-claim-hold` | `CODEHUB_RUNNER_CLAIM_HOLD` | no | Seconds a runner's claim waits for a job. |
| `--runner-heartbeat-interval` | `CODEHUB_RUNNER_HEARTBEAT_INTERVAL` | no | Seconds between the heartbeats runners are told to send. |
| `--runner-lease` | `CODEHUB_RUNNER_LEASE` | no | Seconds a claim or heartbeat holds a job; it must exceed the heartbeat interval or the server refuses to start. |
| `--s3-endpoint` | `CODEHUB_S3_ENDPOINT` | no | The URL of the S3-compatible object store. |
| `--s3-region` | `CODEHUB_S3_REGION` | no | The object store's region. |
| `--s3-bucket` | `CODEHUB_S3_BUCKET` | no | The bucket holding the objects. |
| `--s3-access-key` | `CODEHUB_S3_ACCESS_KEY` | no | The access key. |
| `--s3-secret-key` | `CODEHUB_S3_SECRET_KEY` | yes | The secret key. |
| `--max-object-size` | `CODEHUB_MAX_OBJECT_SIZE` | no | The largest upload, in bytes. |
| `--grant-expiry-seconds` | `CODEHUB_GRANT_EXPIRY_SECONDS` | no | How long an upload or download grant lasts. |
| `--identity` | `CODEHUB_IDENTITY` | no | `oidc` (default) or `introspection`. |
| `--oidc-issuer` | `CODEHUB_OIDC_ISSUER` | no | The accepted issuer; required with `oidc`. |
| `--oidc-audience` | `CODEHUB_OIDC_AUDIENCE` | no | The accepted audience; required with `oidc`. |
| `--jwks-min-refetch-seconds` | `CODEHUB_JWKS_MIN_REFETCH_SECONDS` | no | The shortest time between fetches of the issuer's keys. |
| `--introspection-endpoint` | `CODEHUB_INTROSPECTION_ENDPOINT` | no | The RFC 7662 endpoint; required with `introspection`. |
| `--introspection-client-id` | `CODEHUB_INTROSPECTION_CLIENT_ID` | no | The client id at that endpoint; required with `introspection`. |
| `--introspection-client-secret` | `CODEHUB_INTROSPECTION_CLIENT_SECRET` | yes | The client secret at that endpoint; required with `introspection`. |
| `--introspection-issuer` | `CODEHUB_INTROSPECTION_ISSUER` | no | The issuer named where a response has no `iss`; required with `introspection`. |
| `--introspection-audience` | `CODEHUB_INTROSPECTION_AUDIENCE` | no | The accepted audience; required with `introspection`. |
| `--authorisation` | `CODEHUB_AUTHORISATION` | no | `postgres` (default) or `authzen`. |
| `--authzen-url` | `CODEHUB_AUTHZEN_URL` | no | The AuthZEN decision point; required with `authzen`. |
| `--authzen-token-endpoint` | `CODEHUB_AUTHZEN_TOKEN_ENDPOINT` | no | The token endpoint for the decision point's bearer token; required with `authzen`. |
| `--authzen-client-id` | `CODEHUB_AUTHZEN_CLIENT_ID` | no | The client id there; required with `authzen`. With `private-key-jwt` a template holding `{organisation}`, such as `grn:{organisation}/iam/service/codehub`. |
| `--authzen-client-secret` | `CODEHUB_AUTHZEN_CLIENT_SECRET` | yes | The client secret there; required with `authzen`. |
| `--authzen-evaluations` | `CODEHUB_AUTHZEN_EVALUATIONS` | no | Send Access Evaluations where the decision point offers them (ADR 0041). |
| `--authzen-profile` | `CODEHUB_AUTHZEN_PROFILE` | no | The names the adapter sends: `plain` (default) or `grn`, garam's. `grn` needs `private-key-jwt` (ADR 0043). |
| `--authzen-client-authentication` | `CODEHUB_AUTHZEN_CLIENT_AUTHENTICATION` | no | `client-secret` (default) or `private-key-jwt`, one client and token per organisation (ADR 0042). |
| `--authzen-key-file` | `CODEHUB_AUTHZEN_KEY_FILE` | no | The path of the PEM PKCS#8 RSA or P-256 private key that signs the assertions; required with `private-key-jwt`. The key is read from the file, never from a setting. |
| `--authzen-key-id` | `CODEHUB_AUTHZEN_KEY_ID` | no | The `kid` header of the assertions, such as the key's RFC 7638 thumbprint; optional. |
| `--authzen-issuer` | `CODEHUB_AUTHZEN_ISSUER` | no | The issuer identifier of the token endpoint's authorization server, the assertions' `aud`; required with `private-key-jwt`. |
| `--authzen-audience` | `CODEHUB_AUTHZEN_AUDIENCE` | no | Sent as the token request's `audience` parameter where given. |

## Runner options

The runner is configured by options, and by the variables where the table gives one. It needs one source of tokens: either `--token-file`, or `--token-endpoint` with `--client-id` and `--client-secret-env`, not both. The client secret itself is read from the environment variable that `--client-secret-env` names, so that variable is a secret of the deployment, though no row below holds it.

| Option | Variable | Secret | Purpose |
|---|---|---|---|
| `--server` | `CODEHUB_RUNNER_SERVER` | no | The server's base URL. |
| `--token-file` | `CODEHUB_RUNNER_TOKEN_FILE` | no | A file holding the bearer token, read again before each request; the file's content is a secret. |
| `--token-endpoint` | `CODEHUB_RUNNER_TOKEN_ENDPOINT` | no | The OAuth token endpoint for the client-credentials grant. |
| `--client-id` | `CODEHUB_RUNNER_CLIENT_ID` | no | The OAuth client id. |
| `--client-secret-env` | `CODEHUB_RUNNER_CLIENT_SECRET_ENV` | no | The name of the variable holding the OAuth client secret. |
| `--work-dir` | `CODEHUB_RUNNER_WORK_DIR` | no | Where each job's directory is made; the system's temporary directory where absent. |
| `--once` | - | no | Claim once, run the job, and exit. |

## What the server needs

- **PostgreSQL** reachable at `CODEHUB_POSTGRES_URL`. The server migrates the schema forward at start (ADR 0022), so the user needs the right to create and alter tables in its database.
- **An S3-compatible object store** at `CODEHUB_S3_ENDPOINT`, with the bucket `CODEHUB_S3_BUCKET` already made: the server does not create it. Clients and runners move object bytes to and from the store by presigned URL, so the endpoint must be reachable by them as well as by the server (ADR 0015).
  The identity needs `s3:ListBucket` on the bucket as well as object access: without it, S3 answers a missing key with 403, and `/readyz` stays 503.
- **An identity issuer** the server verifies tokens against, as `CODEHUB_IDENTITY` selects (`docs/architecture/access.md`), and, in the external mode, an AuthZEN decision point.

## Health

Both probes are unauthenticated and sit outside `/v1`.

| Probe | Path | Answers |
|---|---|---|
| Liveness | `GET /healthz` | 200 while the process serves requests; depends on nothing. |
| Readiness | `GET /readyz` | 200 where PostgreSQL and the object store each answer within 2 seconds; otherwise 503, with a problem detail naming the one that did not. A missing bucket is not detected: the probe asks for an object, and a 404 for it is an answer whether the object or the bucket is missing. |

The runner serves no port, so it has no probe.

## How the runner reaches the server

The runner calls the server's HTTP API under `/v1/runner/` at `CODEHUB_RUNNER_SERVER`, outbound only: the server never connects to a runner. It presents a bearer token from the issuer the server accepts, and the principal it names must be registered as a runner of the organisation or repository with the labels its jobs need (`docs/architecture/runner.md`, ADR 0034).
