# Deployment

How to run the codehub server and runner images: what each image holds, what it is configured with, what it needs, and what to probe.

This file is the contract between codehub and whoever declares its deployment. Its two settings tables are compared with `codehub all --help` and `codehub-runner --help` by `crates/codehub/tests/deployment.rs` and `crates/codehub-runner/tests/deployment.rs`, and the Roles column of the server table with each role's own `--help`, so a setting added or removed in code, or moved between roles, fails the tests until this file says so.

## Contents
- Images
- Server roles
- Server settings
- Runner options
- Checking an executor
- The runner on gaon
- What the server needs
- Health
- How the runner reaches the server

## Images

A tag `vX.Y.Z`, `vX.Y.Z-dev.N` or `vX.Y.Z-rc.N` of this repository publishes both images, tagged with the version (the tag without its `v`) and never `latest` (`.github/workflows/release.yml`). N is a positive number with no leading zero. A tag with a suffix makes a GitHub pre-release that is not marked latest; any other tag publishes nothing. Each is built from the repository root with the Dockerfile named below.

| Image | Dockerfile | Entrypoint | Notes |
|---|---|---|---|
| `codehub/server:<version>` | `deploy/server.Dockerfile` | `/usr/local/bin/codehub` | Exposes port 8080. |
| `codehub/runner:<version>` | `deploy/runner.Dockerfile` | `/usr/local/bin/codehub-runner` | Holds `/bin/sh` and coreutils, which the host driver runs a job's command under. |

- Both run as user 10001, not root, and are based on `debian:trixie-slim` with CA certificates.
- Both carry `org.opencontainers.image.version` and `org.opencontainers.image.revision` labels.
- No address, URL or secret is set at build time. Every setting comes from the variables below at run.
- The container takes a role first (`api`, `runner-api`, `worker`, `migrate` or `all`), then that role's options, after the entrypoint; with no role it runs `all`.
- A push that fails between the two images leaves only the server published. The repositories are tag-immutable, so a re-run cannot repair it and the repair is the next version.

## Server roles

The server binary runs as one role, named by its first argument (ADR 0052). The roles talk to each other only through PostgreSQL and the object store, so each can be deployed, scaled and restarted on its own.

| Role | Runs |
|---|---|
| `api` | The `/v1` API of users and agents, except `/v1/runner/`. |
| `runner-api` | The `/v1/runner/` API runners call: claim, heartbeat, result and uploads. |
| `worker` | The background work: delivering the directory outbox (ADR 0044). |
| `migrate` | Applies the migrations the database has not had, and exits 0. |
| `all` | Migrates, then serves every route and delivers the outbox, in one process. |

- With no role, `codehub` runs `all`, so a deployment of one process is unchanged.
- Only `migrate` and `all` change the schema. Run `migrate` once, before `api`, `runner-api` and `worker` of a new version start.
- Each role takes only the options its row in the table below lists, and refuses another with exit code 2. An environment variable of an option a role does not take is ignored.
- `api` and `runner-api` serve the probes below on the listening address, as `worker` does for its own.

## Server settings

Every setting is an option and a variable of the same name, the option's name upper-cased with the prefix `CODEHUB_`. A variable and the option both given: the option wins. A secret is marked `yes` below, and its value is hidden in the help output and in the server's configuration output.

The roles that serve listen on `127.0.0.1:8080` unless `CODEHUB_LISTEN` says otherwise, so in a container set it to an address that is reachable, such as `0.0.0.0:8080`. It stops on SIGTERM or SIGINT after finishing the requests in flight.

| Option | Variable | Secret | Roles | Purpose |
|---|---|---|---|---|
| `--listen` | `CODEHUB_LISTEN` | no | api, runner-api, worker, all | The address to listen on. |
| `--postgres-url` | `CODEHUB_POSTGRES_URL` | yes | api, runner-api, worker, migrate, all | The PostgreSQL database holding the metadata. |
| `--postgres-pool-size` | `CODEHUB_POSTGRES_POOL_SIZE` | no | api, runner-api, worker, migrate, all | Connections each server process holds; the sum over processes stays below PostgreSQL's `max_connections` (ADR 0037). |
| `--landing-attempts` | `CODEHUB_LANDING_ATTEMPTS` | no | api, runner-api, all | How often a landing is tried while `main` moves under it (ADR 0036). |
| `--runner-claim-hold` | `CODEHUB_RUNNER_CLAIM_HOLD` | no | runner-api, all | Seconds a runner's claim waits for a job. |
| `--runner-heartbeat-interval` | `CODEHUB_RUNNER_HEARTBEAT_INTERVAL` | no | runner-api, all | Seconds between the heartbeats runners are told to send. |
| `--runner-lease` | `CODEHUB_RUNNER_LEASE` | no | runner-api, all | Seconds a claim or heartbeat holds a job; it must exceed the heartbeat interval or the server refuses to start. |
| `--s3-endpoint` | `CODEHUB_S3_ENDPOINT` | no | api, runner-api, all | The URL of the S3-compatible object store. |
| `--s3-region` | `CODEHUB_S3_REGION` | no | api, runner-api, all | The object store's region. |
| `--s3-bucket` | `CODEHUB_S3_BUCKET` | no | api, runner-api, all | The bucket holding the objects. |
| `--s3-access-key` | `CODEHUB_S3_ACCESS_KEY` | no | api, runner-api, all | The access key. |
| `--s3-secret-key` | `CODEHUB_S3_SECRET_KEY` | yes | api, runner-api, all | The secret key. |
| `--max-object-size` | `CODEHUB_MAX_OBJECT_SIZE` | no | api, runner-api, all | The largest upload, in bytes. |
| `--grant-expiry-seconds` | `CODEHUB_GRANT_EXPIRY_SECONDS` | no | api, runner-api, all | How long an upload or download grant lasts. |
| `--identity` | `CODEHUB_IDENTITY` | no | api, runner-api, all | `oidc` (default) or `introspection`. |
| `--oidc-issuer` | `CODEHUB_OIDC_ISSUER` | no | api, runner-api, all | The accepted issuer; required with `oidc`. |
| `--oidc-audience` | `CODEHUB_OIDC_AUDIENCE` | no | api, runner-api, all | The accepted audience; required with `oidc`. |
| `--jwks-min-refetch-seconds` | `CODEHUB_JWKS_MIN_REFETCH_SECONDS` | no | api, runner-api, all | The shortest time between fetches of the issuer's keys. |
| `--introspection-endpoint` | `CODEHUB_INTROSPECTION_ENDPOINT` | no | api, runner-api, all | The RFC 7662 endpoint; required with `introspection`. |
| `--introspection-client-id` | `CODEHUB_INTROSPECTION_CLIENT_ID` | no | api, runner-api, all | The client id at that endpoint; required with `introspection`. |
| `--introspection-client-secret` | `CODEHUB_INTROSPECTION_CLIENT_SECRET` | yes | api, runner-api, all | The client secret at that endpoint; required with `introspection`. |
| `--introspection-issuer` | `CODEHUB_INTROSPECTION_ISSUER` | no | api, runner-api, all | The issuer named where a response has no `iss`; required with `introspection`. |
| `--introspection-audience` | `CODEHUB_INTROSPECTION_AUDIENCE` | no | api, runner-api, all | The accepted audience; required with `introspection`. |
| `--authorisation` | `CODEHUB_AUTHORISATION` | no | api, runner-api, all | `postgres` (default) or `authzen`. |
| `--authzen-url` | `CODEHUB_AUTHZEN_URL` | no | api, runner-api, all | The AuthZEN decision point; required with `authzen`. |
| `--authzen-token-endpoint` | `CODEHUB_AUTHZEN_TOKEN_ENDPOINT` | no | api, runner-api, worker, all | The token endpoint for the decision point's bearer token; required with `authzen`. |
| `--authzen-client-id` | `CODEHUB_AUTHZEN_CLIENT_ID` | no | api, runner-api, worker, all | The client id there; required with `authzen`. With `private-key-jwt` a template holding `{organisation}`, such as `grn:{organisation}/iam/service/codehub`. |
| `--authzen-client-secret` | `CODEHUB_AUTHZEN_CLIENT_SECRET` | yes | api, runner-api, worker, all | The client secret there; required with `client-secret`, unused with `private-key-jwt`. |
| `--authzen-evaluations` | `CODEHUB_AUTHZEN_EVALUATIONS` | no | api, runner-api, all | Send Access Evaluations where the decision point offers them (ADR 0041). |
| `--authzen-profile` | `CODEHUB_AUTHZEN_PROFILE` | no | api, runner-api, worker, all | The names the adapter sends: `plain` (default) or `grn`, garam's. `grn` needs `authzen` and `private-key-jwt` (ADR 0043). |
| `--authzen-client-authentication` | `CODEHUB_AUTHZEN_CLIENT_AUTHENTICATION` | no | api, runner-api, worker, all | `client-secret` (default) or `private-key-jwt`, one client and token per organisation (ADR 0042). |
| `--authzen-key-file` | `CODEHUB_AUTHZEN_KEY_FILE` | no | api, runner-api, worker, all | The path of the PEM PKCS#8 RSA or P-256 private key that signs the assertions; required with `private-key-jwt`. The key is read from the file, never from a setting. |
| `--authzen-key-id` | `CODEHUB_AUTHZEN_KEY_ID` | no | api, runner-api, worker, all | The `kid` header of the assertions, such as the key's RFC 7638 thumbprint; optional. |
| `--authzen-issuer` | `CODEHUB_AUTHZEN_ISSUER` | no | api, runner-api, worker, all | The issuer identifier of the token endpoint's authorization server, the assertions' `aud`; required with `private-key-jwt`. |
| `--authzen-audience` | `CODEHUB_AUTHZEN_AUDIENCE` | no | api, runner-api, worker, all | Sent as the token request's `audience` parameter where given. |
| `--directory-changes` | `CODEHUB_DIRECTORY_CHANGES` | no | api, worker, all | Where repository and organisation changes go: `none` (default) or `garam`, garam's `ApplyEntityChanges` with the organisation's token. `garam` needs `CODEHUB_AUTHZEN_PROFILE=grn`, or the server refuses to start (ADR 0044). `none` accepts and discards every entry the outbox holds, so enable `garam` before creating any repository garam must know: one created under `none` is never sent later. |
| `--directory-garam-url` | `CODEHUB_DIRECTORY_GARAM_URL` | no | api, worker, all | The base URL of garam's policy service, which the entity changes are sent to; required with `garam`. |
| `--directory-delivery-wait` | `CODEHUB_DIRECTORY_DELIVERY_WAIT` | no | api, all | Seconds, at most 3600, creating a repository waits for its change to reach the receiver, 2 by default; the creation is answered either way, and an undelivered change is retried. |

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
| `--object-cache-size` | `CODEHUB_RUNNER_OBJECT_CACHE_SIZE` | no | The most bytes of files the host executor keeps in `objects/` under the work directory between jobs, default 10737418240 (10 GiB); after each job the least recently used are removed until it fits. |
| `--once` | - | no | Claim once, run the job, and exit. |
| `--executor` | `CODEHUB_RUNNER_EXECUTOR` | no | Where a job's command runs: `host` (default), a process of the runner's machine, or `gaon`, a run-once MicroVM on gaon (ADR 0046). `gaon` needs the seven gaon options below, and the runner refuses to start without any one. |
| `--gaon-url` | `CODEHUB_RUNNER_GAON_URL` | no | The gaon API server's root URL, `https://` and ending in `/apis/compute.gaon.garam.sh/v1alpha1`; the runner sends its token only over https. |
| `--gaon-ca-file` | `CODEHUB_RUNNER_GAON_CA_FILE` | no | The PEM file of the certificate authority that signed the API server's certificate; it is the only authority the runner trusts for gaon. |
| `--gaon-token-file` | `CODEHUB_RUNNER_GAON_TOKEN_FILE` | no | A file holding the gaon service-account token, read again before each request; the file's content is a secret. |
| `--gaon-image` | `CODEHUB_RUNNER_GAON_IMAGE` | no | The image jobs run in, `<repository>@sha256:<digest>`: a digest, never a tag. It holds `codehub-runner` and what the jobs' commands need. |
| `--gaon-result-listen` | `CODEHUB_RUNNER_GAON_RESULT_LISTEN` | no | The address the runner listens on, in plain HTTP, for the results of jobs, such as `0.0.0.0:8090`. |
| `--gaon-result-url` | `CODEHUB_RUNNER_GAON_RESULT_URL` | no | The URL a job's MicroVM reaches that listener at. |
| `--gaon-runner-name` | `CODEHUB_RUNNER_GAON_RUNNER_NAME` | no | The value of the label `runner.codehub.garam.sh/name` on every MicroVM and Secret this runner creates: 1 to 63 alphanumerics, `-`, `_` or `.`, beginning and ending with an alphanumeric, and unique among the runners of one gaon cluster. The runner deletes every object that carries it when it starts. |
| `--gaon-job-user` | `CODEHUB_RUNNER_GAON_JOB_USER` | no | The numeric `uid:gid` a job's MicroVM runs as (`spec.user`), default `10001:10001`, the user of the runner image. The MicroVM's description file is owned by it; gaon makes a file without an owner root-owned, which the image's user could not read, and refuses a user given by name. |

`codehub-runner run-job` is the mode a job's MicroVM runs under gaon: it takes the path of the job file, needs neither the server nor credentials, and is not for a person to run.

## Checking an executor

`codehub-runner check-executor --command <command> [--timeout <seconds>]` runs one job with no files and that command through the executor that `--executor` and its settings configure (`--work-dir` for `host`, the gaon options for `gaon`), and exits with the command's code. An operator runs it to check an executor's setup before registering a runner (ADR 0047). It contacts no codehub server, so it needs neither `--server` nor a token setting. It stops the job as timed out after `--timeout` seconds, 600 by default, prints one line naming how the job ended and then the output tail, and exits 1 for an end that is not a code from 0 to 255 and 125 for an executor error. With `--executor gaon` it removes the objects that carry `--gaon-runner-name` when it starts, as the runner does, so give it a name of its own and never a live runner's: that runner's job in flight would be deleted.

## The runner on gaon

With `--executor gaon` the runner creates, reads, lists and deletes MicroVMs and Secrets of the group `compute.gaon.garam.sh` as the identity its token names. Give that identity a ClusterRole and a ClusterRoleBinding that allow `create`, `get`, `list` and `delete` on `microvms` and `secrets` in `compute.gaon.garam.sh`, each rule with a `labelSelector` on `runner.codehub.garam.sh/name` equal to the runner's `--gaon-runner-name`, so that the identity reaches only the objects of its own runner. The runner lists only with that selector.

Egress and ingress are the cluster's. A job's MicroVM must reach the object store's download URLs and the result URL, and the runner's own MicroVM must declare the port of `--gaon-result-listen` in its `spec.ports`, or the runner cannot receive a result. A job whose MicroVM cannot post its result ends as an executor error naming the MicroVM's final phase.

## What the server needs

- **PostgreSQL** reachable at `CODEHUB_POSTGRES_URL`. `migrate` and `all` migrate the schema forward (ADR 0022), so the user they connect as needs the right to create and alter tables in its database; the other roles need only the rights to use it.
- **An S3-compatible object store** at `CODEHUB_S3_ENDPOINT`, with the bucket `CODEHUB_S3_BUCKET` already made: the server does not create it. Clients and runners move object bytes to and from the store by presigned URL, so the endpoint must be reachable by them as well as by the server (ADR 0015).
  The identity needs `s3:ListBucket` on the bucket as well as object access: without it, S3 answers a missing key with 403, and `/readyz` stays 503.
- **An identity issuer** the server verifies tokens against, as `CODEHUB_IDENTITY` selects (`docs/architecture/access.md`), and, in the external mode, an AuthZEN decision point.

## Health

Every role that serves (`api`, `runner-api`, `worker` and `all`) answers both probes on its listening address. They are unauthenticated and sit outside `/v1`.

| Probe | Path | Answers |
|---|---|---|
| Liveness | `GET /healthz` | 200 while the process serves requests; depends on nothing. |
| Readiness | `GET /readyz` | 200 where PostgreSQL answers within 2 seconds with a schema at least as new as the binary's latest migration, and, for `api`, `runner-api` and `all`, the object store answers within 2 seconds; otherwise 503, with a problem detail naming the one that did not. `api`, `runner-api` and `worker` do not migrate, so on a database that `migrate` has not brought forward they stay not ready; a newer schema is accepted, so a rolling upgrade can run the old binary beside the new one. A missing bucket is not detected: the probe asks for an object, and a 404 for it is an answer whether the object or the bucket is missing. |

With the host driver the runner serves no port; with the gaon driver it serves only the result listener, which is not a probe. Either way it has no probe.

## How the runner reaches the server

The runner calls the server's HTTP API under `/v1/runner/` at `CODEHUB_RUNNER_SERVER`, outbound only: the server never connects to a runner. With the roles split, route `/v1/runner/` to `runner-api` and every other path under `/v1` to `api`; `api` answers 404 for the runner's paths and `runner-api` for every other `/v1` path. It presents a bearer token from the issuer the server accepts, and the principal it names must be registered as a runner of the organisation or repository with the labels its jobs need (`docs/architecture/runner.md`, ADR 0034).
