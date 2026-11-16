# ADR 0057: the Helm chart deploys each role as a Deployment, and kind fault runs gate it

> Status: accepted
> Date: 2026-11-16

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0052 splits the server into roles that share only PostgreSQL and the object store, ADR 0053 puts the image registry beside them, and goal #273 asks for the whole to run on Kubernetes. A chart that renders is not a deployment that survives its faults: a pod killed under load, a PostgreSQL failover, a registry behind several replicas and a restore from backup are the cases a production deployment meets, and the claim that no acknowledged landing is lost is only as good as a test that kills things while landings are acknowledged.

Two choices were considered and not taken:

- **A chart with one Deployment running `all`.** It scales the API, the runner API and delivery together and migrates at every start, which ADR 0052 split the roles to avoid.
- **Fault tests as a mock of the cluster** (a fake client that returns errors). They test the harness's idea of a failure, not PostgreSQL's promotion, a pooler's connection handling or a pod's termination.

## Decision

- **The chart** is `deploy/helm/codehub`. Each of `api`, `runner-api` and `worker` is a Deployment with its own replicas, resources, HorizontalPodAutoscaler, PodDisruptionBudget, pod anti-affinity (soft by default, hard on request) and startup, liveness and readiness probes on `/healthz` and `/readyz`. `api` and `runner-api` have a Service; the worker serves only its probes and has none. One ConfigMap holds the settings every role shares, and each role takes what it uses (ADR 0052).
- **Migration** is `codehub migrate` in a Job that Helm runs as a `pre-install,pre-upgrade` hook, so a new version's pods find the schema they need and `/readyz` (ADR 0052) is only a guard. The Job reads the primary directly: a pooler in transaction mode cannot hold the advisory lock the migrations take, so `postgres.migrateUrlSecret` names a direct URL and defaults to the pods' URL.
- **No secret is a value.** Every credential is a Secret that exists beforehand, named by `name` and `key`: the PostgreSQL URLs (primary, migrate, read replica), the S3 secret key, the registry's `http.secret`, the signing key and its JWKS, the runner's client secret, and any other secret setting through `secretEnv`. The chart's values and its rendered objects carry no secret.
- **PostgreSQL and S3 are values.** The chart installs neither. The URL is the one the operator gives, a pooler's Service in transaction mode included; `postgres.statementCache` is rendered as `CODEHUB_POSTGRES_STATEMENT_CACHE` (1 or more behind a pooler in transaction mode, which must track prepared statements: ADR 0054), `postgres.readUrlSecret` as `CODEHUB_POSTGRES_READ_URL`, and `postgres.poolSize` bounds each pod's connections, so the sum over every pod stays below `max_connections` (ADR 0037).
- **The registry** is a Deployment of CNCF Distribution, pinned by tag and digest, with replicas, an autoscaler and a budget of its own. Its replicas share one `http.secret`, so an upload begun on one finishes on another; its state is the bucket. It trusts the JWKS in the chart's key Secret with `ES256`, and its token realm is codehub's endpoint (ADR 0053). `api` mounts the key's PEM to sign with.
- **A pod outlives its shutdown** (ADR 0055): each role's `terminationGracePeriodSeconds` exceeds `CODEHUB_SHUTDOWN_DEADLINE_SECONDS` (25 by default, set in `config`), and the chart fails to render where it does not. The metrics of every role (ADR 0056) are scraped by pod annotations; the chart offers no ServiceMonitor. The ingress passes the runner routes' bodies of up to 64 MiB where the controller's body limit allows it, which the values document for ingress-nginx.
- **Examples** ship off by default: a runner Deployment that polls the runner API, and a KEDA `ScaledObject` that scales it on the queue depth, a Prometheus query given as a value (`sum(codehub_jobs_queued)` by default).
- **The chart is checked on every `make ci`** by `make helm-check`: `helm lint --strict` and `helm template` over every set of values in `deploy/helm/codehub/ci/`, which reports the sets and objects and fails where a set renders none.
- **Fault runs** are `make kind-faults`, driven by `deploy/helm/tests/kind-faults.sh`, in a kind cluster of one control plane and four workers with CloudNativePG (three instances and a PgBouncer `Pooler` in transaction mode), distributed MinIO, Dex, the chart and the registry. They are not part of `make ci`; a manual-dispatch workflow runs them. The load is `crates/loadtest` against the running server: agents land small files and survive every failure; each acknowledged landing is recorded; after the load, every one is read back from the server, its history and its bytes. A scenario passes where no acknowledged landing is missing or differs, and landings succeed again within its limit after the fault. The five scenarios and their limits:
  - an API pod killed under load, 30 seconds;
  - the PostgreSQL primary killed under load, a new primary promoted, 60 seconds;
  - the same through the pooler in transaction mode, with the default statement cache and a pooler that tracks prepared statements, 75 seconds;
  - the registry scaled to 3 replicas, 30 pushes and pulls, every replica serving requests;
  - a restore drill: a CloudNativePG backup to MinIO, then a copy of the S3 bucket, then more landings, then a restore into a new cluster and a new bucket; what landed before the backup must be present and intact, what landed after may be absent but must be intact where present, so no landing refers to a missing object.
- **The limits are sums of their parts**, derived from the settings in force and not from what a run showed. A limit that a run exceeds is raised only with the part that explains it.
  - API pod killed, 30 seconds: the Service stops routing to the pod once its endpoint is removed, within one kube-proxy sync (under 5 seconds); the second replica serves meanwhile, so the client sees at most one failed attempt and retries after 250 milliseconds. Were the survivor not enough, a replacement is ready within the startup probe period (2 seconds) plus the readiness period (5 seconds) plus the start of the process (under 10 seconds). The rest is slack.
  - Primary killed, direct, 60 seconds: 30 seconds for CloudNativePG to see the loss, promote the most advanced replica (end-of-recovery checkpoint included) and for the read-write Service to point at it; plus up to 6 seconds for the server to notice (the supervisor's one-second poll and its 5-second probe of a host that does not answer, ADR 0054); plus about 20 seconds, the longest a request that began in the gap spends: 4 attempts, each waiting the 5-second default `CODEHUB_POSTGRES_ACQUIRE_TIMEOUT`, and 350 milliseconds of backoff; the client then retries after 250 milliseconds. The sum is about 56 seconds, so 60 holds. (Before #300 the acquire timeout was sqlx's 30 seconds and the limit counted one such wait; the new default shortens the longest wait but multiplies it by the retries, and the limit is unchanged.)
  - Primary killed, through the pooler, 75 seconds: the 60 above, plus 15 seconds, PgBouncer's default `server_login_retry`, the wait after a login to the not-yet-promoted server fails before it tries again. The load window is 120 seconds so that it outlasts the limit. The pooler runs with `max_prepared_statements` 200 and `ignore_startup_parameters` `options`, as ADR 0054 requires: without prepared-statement tracking the server's unnamed statements failed through the pooler with `unnamed prepared statement does not exist`, as a 500 on about one attempt in seven from the first request and with no fault, which is how the earlier runs of this scenario failed.
  - The rate limit is lifted in the runs (`CODEHUB_RATE_LIMIT_PER_SECOND` 5000, burst 10000), a limit raised with its reason: the load is one caller, and ADR 0055's default of 100 requests a second per caller answered it with 429 (219 of 228 failed attempts in one 60-second run, growing with the load, and the read-back's reads among them), which measures the limit and not the fault. The limit has its own tests.
  - The database runs with synchronous replication (CloudNativePG `postgresql.synchronous`, one standby of any), as ADR 0054 requires for zero loss. A run on a host under load (load average above 25) without it lost 40 of 344 acknowledged landings at the failover: the primary had acknowledged commits that no standby had yet received. That is the asynchronous default behaving as documented, and the claim "no acknowledged landing is lost" holds only with the setting.

## Consequences

- A release upgrade migrates first, then rolls the roles one by one; an old `api` keeps serving until the new schema is applied and a new one is ready only once it is.
- The chart assumes names that the issues of goal #273 define and that are not merged when this ADR is: `CODEHUB_POSTGRES_READ_URL`, `CODEHUB_POSTGRES_STATEMENT_CACHE`, the `CODEHUB_REGISTRY_*` settings and the `codehub_` queue-depth metric. They are rendered from one place each, and are reconciled with the merged names before the goal is merged.
- The registry scenario needs the token endpoint of ADR 0053, so it cannot pass before that is merged.
- The loadtest binary gains a mode that loads a running server and verifies a record of acknowledged landings; the local scenarios are unchanged.
- The kind cluster reaches Dex, MinIO and the API at the node's address on NodePorts, since a port-forward ends with the pod it forwards to.
- A restore is a drill on a fixed layout, not a procedure for every backup tool; an operator with another tool repeats the drill with it.

## Signs the decision was wrong

- A value an operator must set is one the chart could derive, so the same few lines are copied into every release.
- A role needs a setting the shared ConfigMap cannot carry without leaking it to a role that must not have it.
- The fault runs are too slow or too flaky to run before a release, and are not run, so the claim they back is unchecked.
- A fault the runs do not inject, such as a lost node or a partition between roles and PostgreSQL, loses an acknowledged landing in production.
- Kubernetes manifests of another form, such as Kustomize, are what operators use, and the chart is not.
