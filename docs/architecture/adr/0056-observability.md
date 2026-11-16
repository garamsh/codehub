# ADR 0056: the server exports metrics at `/metrics` and traces over OTLP, and logs structured lines that name their trace

> Status: accepted
> Date: 2026-11-16

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

Goal #273 runs the server as roles on Kubernetes (ADR 0052). An operator of that deployment needs three things the server did not give: numbers a scraper can collect (how long requests and the landing gate take, how deep the job queue is, how the database pool is used), a trace that follows one request through the server's calls to its dependencies, and log lines a collector can parse and join to that trace. Until now the server logged nothing to standard output.

Alternatives not taken:

- **`prometheus-client` for the metrics.** It keeps its metric families in a registry the code passes around. A landing's gate time is measured in `app`, a layer that must not hold the registry of a binary, so every measuring site would need the registry injected. `metrics` is a facade: `app` and the adapters record through macros, and the binary alone installs the recorder (`metrics-exporter-prometheus`) that renders them.
- **The OpenTelemetry metrics SDK, exporting metrics over OTLP.** The Helm chart's scrape configuration is a Prometheus one, and a scrape needs no collector. Traces go over OTLP because a trace is pushed by the process that holds its spans, and nothing scrapes it.
- **Spans through `opentelemetry` alone, without `tracing`.** The code already reports through no facade, and `tracing` gives a span that follows an `async` call across `await` points and a log line that knows the span it is written in. `tracing-opentelemetry` turns those spans into OpenTelemetry spans.
- **Timing every PostgreSQL statement.** `sqlx` has no hook around a statement, so timing them would mean wrapping every query of `adapter-postgres`. The request span and the gate and landing spans show a slow statement as a slow step, and the pool is measured on its own (below).
- **The pool's acquire time from the pool.** `sqlx` does not report how long an acquire waited, so a sampler takes a connection every few seconds and records how long that took. It is a probe of the pool's wait, not a measure of every acquire.
- **`/metrics` under `/v1` with a token.** The reasons ADR 0045 gives for the probes apply: a scraper holds no token, and `/v1` is the versioned, authenticated API.
- **gRPC or TLS for the OTLP export.** Either brings `tonic` and a second TLS stack into the lockfile. The export is HTTP, and a collector reached over TLS is reached through the collector the cluster runs beside the pod.

## Decision

### Crates

Added to the lockfile, all for this issue:

- `metrics` and `metrics-exporter-prometheus` (without its HTTP listener, which is not used): the metrics facade and the recorder that renders the Prometheus text format.
- `tracing`, `tracing-subscriber` (`env-filter`, `fmt`, `json`, `registry`): spans, log lines and the filter.
- `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp` (`http-proto`, `http-json`, `reqwest-blocking-client`), `opentelemetry-http` and `tracing-opentelemetry`: the span export and the `traceparent` header.

`opentelemetry-otlp` uses its blocking `reqwest` client, which is the `reqwest` already in the lockfile; the lockfile holds one `reqwest`.

### `/metrics`

- `GET /metrics` is served at the root of every role that serves, the worker included, on the role's listening address. It is outside `/v1` and needs no token, for the reasons ADR 0045 gives the probes. It is in the OpenAPI document, and the test that asserts every route is documented covers it.
- It answers the Prometheus text format and is not itself a request the server measures: neither it nor the probes pass through the request middleware.
- It discloses counts and durations and the labels below, and no organisation, repository, user or credential name: a route is its template, never its path.

### Metrics

| Metric | Type | Labels | Emitted |
|---|---|---|---|
| `codehub_http_request_duration_seconds` | histogram | `method`, `route` (the template), `status` | the request middleware of `adapter-http`, once per API request |
| `codehub_landing_gate_wait_seconds` | histogram | | `source_control::land`, from arriving at a repository's gate to passing it |
| `codehub_landing_gate_hold_seconds` | histogram | | `source_control::land`, from passing the gate to leaving it |
| `codehub_landing_attempts` | histogram | | `land`, once per landing, the attempts it made |
| `codehub_landing_swaps_lost_total` | counter | | `land`, the compare-and-swaps lost to a `main` that moved first |
| `codehub_outbound_call_duration_seconds` | histogram | `dependency` (`identity`, `decision_point`, `object_store`, `directory`) | the `Observed` decorator, once per call |
| `codehub_outbound_call_errors_total` | counter | `dependency` | the `Observed` decorator, where the call failed |
| `codehub_queue_depth` | gauge | `kind` (`job`, `run`), `labels` | the backlog sampler, queued jobs and runs; a label set that has no queued entry any more is set to 0 |
| `codehub_queue_oldest_age_seconds` | gauge | `kind`, `labels` | the backlog sampler, how long the oldest queued entry has waited since it was last updated |
| `codehub_outbox_pending` | gauge | | the backlog sampler, directory outbox entries not delivered |
| `codehub_db_pool_connections` | gauge | | the pool sampler, connections open |
| `codehub_db_pool_connections_in_use` | gauge | | the pool sampler, connections lent out |
| `codehub_db_pool_acquire_seconds` | histogram | | the pool sampler, the time to take a connection, probed every 5 seconds |

The backlog sampler runs in the `worker` and `all` roles, the roles that run background work, every `--backlog-interval` seconds (15 by default); the pool sampler runs in every role that holds a pool and serves. The queue and outbox gauges are therefore absent from `api` and `runner-api`; they are read from the worker's or the `all` process's scrape, and a deployment that scrapes every pod sees each of them once per worker.

An identity verification is timed whole, so for OIDC it includes the local check of a token's signature as well as a fetch of the issuer's keys; only a failure of the verifier (`Backend`) counts as an error, not a credential it rejects. An object store call counts as an error only for a `Backend` failure. A directory change not accepted counts as an error.

### Spans

| Span | Emitted |
|---|---|
| `<METHOD> <route template>` (`http.request`, with `http.method`, `http.route`, `http.status_code`) | the request middleware; continues the trace of an incoming `traceparent` header |
| `identity.verify`, `decision_point.decide`, `decision_point.decide_all`, `object_store.put`, `object_store.get`, `object_store.size`, `object_store.exists`, `directory.repository_changed`, `directory.organisation_changed` | the `Observed` decorator, a child of the span of the call it is made under |
| `landing.gate_wait`, `landing.attempt`, and under an attempt `landing.read_main`, `landing.rebase`, `landing.derive_trees`, `landing.put_trees`, `landing.append`, `landing.swap` | `land`, so a slow landing shows which step was slow |

The runner sends a `traceparent` header with every call it makes to the server, so the server's spans for one job's heartbeats and its result are one trace: a call for a job takes its trace id from the job's identifier, and a claim starts a trace. The runner exports no spans of its own.

### Logs

- A request leaves one line, target `codehub::http`, with the method, the route template, the status and the duration. Headers, queries and bodies are never logged, so a credential cannot reach the log through a request.
- Lines are JSON, one object per line, on standard output, and name the span they were written in, including its `trace_id`. `--log-format text` writes human-readable lines instead. `--log-filter` (`CODEHUB_LOG`) takes a level or `target=level` directives, `info` by default.
- A span always has a trace id, exported or not, so a log line names its trace in a deployment that exports none.

### Trace export

- Spans are exported only where the environment names an OTLP endpoint (`OTEL_EXPORTER_OTLP_ENDPOINT` or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`). The protocol is `http/protobuf`, or `http/json` where `OTEL_EXPORTER_OTLP_PROTOCOL` (or `..._TRACES_PROTOCOL`) names it; any other value is a startup error. `OTEL_SERVICE_NAME` names the service (`codehub` by default); the resource also carries `codehub.role`. The other `OTEL_*` variables the SDK reads (batch delay, headers, timeout) are honoured as it reads them. The variables are read from the environment and are not options of `codehub`.
- Spans not yet exported are flushed when the process stops on a signal.

### Decorators

The calls to the dependencies are measured by one generic `Observed` decorator over the ports (`AuthorisationDecision`, `IdentityVerification`, `ObjectStore`, `DirectoryChanges`) in `app`, applied where `codehub` builds the adapters, so no adapter and no use case knows it is measured. The readiness probe asks the object store itself and not the decorated one: a probe every few seconds would be a trace of its own and would skew the store's latency.

## Consequences

- An operator scrapes any role without a credential, and joins a log line to a trace by `trace_id`.
- `/metrics` is an unauthenticated route that discloses load. It names no tenant data, but anyone who reaches the pod's port reads it; the network, not the server, decides who does.
- The export has no gRPC and no TLS of its own; a collector on another host is reached through one beside the pod.
- No statement-level database timing exists, and the pool's wait is a probe every 5 seconds, so a short stall between probes does not show.
- A histogram's buckets are fixed in the binary (1 ms to 30 s; 1 to 32 for attempts).
- `codehub_queue_depth` has a label set per distinct runner label set queued, so the series count follows the labels the checks name.
- The deployment guide's settings table names `--log-format`, `--log-filter` and `--backlog-interval`.

## Signs the decision was wrong

- Scrapes of `/metrics` are asked to be authenticated, or the metrics begin to carry a tenant's names, and the unauthenticated route has to be put behind a token or its own port.
- Operators need the pool's acquire time per request or per statement, and the probe's samples hide the stalls they are chasing.
- The collector in a deployment cannot take HTTP, and the export needs gRPC or TLS in the process.
- The label sets of the queue gauges grow with the runners' labels until the scrape is large.
