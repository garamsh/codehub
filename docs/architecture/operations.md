# Operations

How the server is run in production: what bounds a process under load, what it reports about itself, how the chart deploys its roles, and how the claim that no acknowledged landing is lost is tested. Roles and PostgreSQL modes are `ADR 0052` and `ADR 0054`; this file holds what an operator and a reviewer of the deployment need to know now.

## Current decisions

### Bounds on a process

- **Every limit is a setting of the serving roles** (`api`, `runner-api`, `all`), per process, with a default that a single process under the checks never meets (ADR 0055).
- **Rate:** a token bucket per authenticated principal on the user API, 100 requests a second with a burst of 200; over it is 429 with `Retry-After`. Runner routes are not rate limited.
- **Size:** 1 MiB for a request body; the runner routes read up to 64 MiB, fixed. More is 413.
- **Time:** 60 seconds for a request, answered 503. The runner's claim, a landing and the runner's result are exempt from this bound and each has its own.
- **Concurrency:** 512 requests in flight per process, answered 503 with `Retry-After`. `/healthz`, `/readyz` and `/metrics` are outside every limit, so a busy process stays ready.
- **Calls a process makes** carry timeouts taken from the adapter's constructor: identity 10 s, decision point 10 s (at most 14), object store connect 3 s and read 10 s. A timeout is a 5xx, never a refusal of the caller.
- **Shutdown:** on SIGTERM the process answers `/readyz` with 503, goes on serving for a fifth of the deadline (at most 5 s), then stops accepting and finishes what is in flight until 25 s. The chart's grace period of each role exceeds that and the chart does not render where it does not.
- **PostgreSQL** pool, acquire and statement timeouts are ADR 0054's and are not repeated here.

### What the server reports

- **Metrics** are served at `/metrics` on every role's own port, Prometheus text format, no token (ADR 0056): request duration by method, route template and status; the landing gate's wait and hold, attempts and lost swaps; each outbound call's duration and errors by dependency; the database pool's connections and a probed acquire time; and the backlog (`codehub_queue_depth`, `codehub_queue_oldest_age_seconds`, `codehub_outbox_pending`).
- **The backlog gauges come from the `worker` and `all` roles only**, and every worker reports the whole backlog, not a share. Whatever sums them over pods multiplies the depth by the worker count.
- **Label sets of the queue gauges are bounded**: the 16 deepest per kind keep a series, the rest are folded into `other`, and the recorder forgets a series nothing has set for 5 minutes. The label string is a tenant's and is readable on the unauthenticated scrape.
- **Traces** are exported over OTLP, HTTP only, where the environment names an endpoint; a request continues an incoming `traceparent`, and the runner sends one with each call. **Logs** are one JSON line a request on standard output, naming the trace; no header, query or body is logged.

### The chart

- **`deploy/helm/codehub`** deploys `api`, `runner-api` and `worker` each as a Deployment with replicas, resources, an optional autoscaler, a disruption budget, pod anti-affinity and probes; `migrate` is a Job Helm runs before an install and an upgrade, on a direct PostgreSQL URL (ADR 0057).
- **No secret is a value:** each credential is an existing Secret named by name and key. PostgreSQL and S3 are values: the chart installs neither.
- **The pool is sized per pod:** `postgres.poolSize` is 10 and the sum over every pod that can run at once must stay below `max_connections` (ADR 0037); `values.yaml` states the sum.
- **Pods run non-root with a read-only root filesystem** and an `emptyDir` at `/tmp`.
- **Routing** is an `Ingress`, a Gateway API `HTTPRoute` or both, each off by default and independent; both send `/v1/runner/` to `runner-api` and the rest to `api`, and the registry's own `HTTPRoute` sends `/v2/` to it (ADR 0058). The 64 MiB body limit is the cluster's to set on the controller or the gateway; `ci/full.yaml` sets it for ingress-nginx.
- **`authzen.keySecret`** mounts the `private-key-jwt` key read-only into `api`, `runner-api` and `worker` and sets `CODEHUB_AUTHZEN_KEY_FILE`; a rotated key takes a rollout (ADR 0058). The Secret contract is in `docs/guide/deployment.md`.
- **The registry** is a Deployment of Distribution with replicas sharing one `http.secret` and the bucket (`registry.md`).
- **Examples, off by default:** a runner Deployment, and a KEDA `ScaledObject` scaling it on `max(sum by (pod) (codehub_queue_depth{kind="job"}))`, which takes the deepest worker's reading.
- **`make helm-check`** lints and renders every set of values in `ci/` and is part of `make ci`.

### Fault runs

- **`make kind-faults`** (not part of `make ci`; a manual-dispatch workflow runs it) kills things under load in a kind cluster with CloudNativePG (three instances, synchronous replication, a PgBouncer pooler), MinIO, Dex, the chart and the registry, and reads every acknowledged landing back (ADR 0057). A scenario passes where none is missing, changed or dangling and landings succeed again within its limit; recovery counts only an unanswered request or a 5xx, and needs a success at or after the fault.
- **The five scenarios and their limits:** an API pod force-killed, 30 s; the primary killed, 60 s; the same through the pooler, 75 s (60 plus PgBouncer's 15 s `server_login_retry`); the registry at three replicas, each serving uploads and blob reads; a restore drill, where what landed before the backup is intact and what landed after is absent or intact with its object.
- **A restore takes the database to a point no later than the copy of the objects.** The objects are copied after the base backup, so recover to the base backup's consistency point (`targetImmediate`) or to a target time at or before the object copy. Replaying all archived WAL past the object copy leaves landings that refer to missing objects, which the drill counts as dangling.
- **The limits are sums of their parts** derived from the settings in force, and are raised only with the part that explains it. The runs lift the rate limit (a test setting, not a bound) and set `synchronous` replication, without which a failover loses acknowledged landings.
- **A run fails with evidence:** the events and every pod's logs are printed before its cluster is deleted, and it refuses to start where a cluster of its name exists.

## Rationale

- Limits are per process and refuse quickly with a status the caller can act on, instead of queueing (ADR 0055); a shared limit needs a shared store the project does not run.
- Metrics are scraped and traces pushed, because a scrape needs no collector and a span lives in the process that holds it; `app` records through facades so it holds no registry (ADR 0056).
- A Deployment per role is what ADR 0052's split is for; the chart installs no database so the operator's choice of PostgreSQL mode stays theirs (ADR 0054, ADR 0057).
- A fault run is real PostgreSQL promotion, a real pooler and real pod termination, since a mock of the cluster tests only the harness's idea of a failure (ADR 0057).

## Open questions

- A limit across replicas, if operators need one: the load balancer's, or a shared store.
- A graceful drain and a rolling upgrade under load are not exercised by any run; the API scenario force-kills a pod.
- Faults the runs do not inject: a lost node, a partition between a role and PostgreSQL.
- That CloudNativePG keeps `synchronous_standby_names` right across a promotion is observed in the runs, not read from its documentation.
- Statement-level database timing, and a pool acquire time per request, do not exist (ADR 0056).
- A runner with a trace of its own: the runner exports no spans.
