# Runner

How a runner joins the system, receives jobs and runs them: registration, polling, labels, pools and the job lifecycle.

## Current decisions

### Model

- **Runners follow the GitHub Actions self-hosted model in shape** (ADR 0009): a runner polls outbound for jobs, the server never connects to a runner, jobs are routed by labels, and runners can run outside the server's host.
- **A runner is a registered principal** (ADR 0034). It calls with a bearer token from the configured issuer, verified like any caller (`access.md`). An organisation admin registers a principal (issuer and subject) as a runner of the organisation, or of one repository in it, with a set of labels. Registering, removing and listing take the administer-runners action on the organisation. There is no registration token, no runner credential issued by codehub, and no mutual TLS.
- **The runner speaks the HTTP JSON API under `/v1/runner/`** (ADR 0034):
  - a long-poll claim the server holds for a configured time, answered with a job or with none;
  - a heartbeat while the job runs, answered with continue or stop (cancelled or timed out);
  - a result report that ends the job.
- **Routing.** A job carries its organisation, repository and labels. It goes only to a runner registered for that organisation or that repository, whose labels include every label of the job. A principal with no registration is refused.
- **The runner is the binary `codehub-runner`**, a package beside `ch` (library and binary), holding the runner core, its two ports and their adapters. It authenticates with a token read again from a file before each request, or by the OAuth client-credentials grant with the secret read from a named environment variable, never logged and not passed into the job command's environment (the host driver does not keep it from a command run as the same user). Its options are `--server`, `--work-dir`, `--once` and `--executor` (`host` or `gaon`, `execution.md`; ADR 0046), with the gaon driver's settings, `codehub-runner run-job` is the single-job mode the gaon driver runs inside a MicroVM, and `codehub-runner check-executor --command <command>` runs one job with no files and that command through the configured executor, contacting no codehub server, and exits with the command's code, so that an operator can check an executor's setup before registering a runner (ADR 0047). The server's `CODEHUB_RUNNER_CLAIM_HOLD`, `CODEHUB_RUNNER_HEARTBEAT_INTERVAL` and `CODEHUB_RUNNER_LEASE` settings configure the claim hold, the heartbeat interval it asks of runners, and the lease; `codehub` refuses to start where the lease does not exceed the heartbeat interval.

### Job lifecycle

A job is queued with the post-land checks of a landing (`source-control.md`, ADR 0035), then:
- **claimed** by a matching runner, which holds a lease renewed by heartbeats;
- **returned to the queue** when the lease passes without a heartbeat;
- **ended as timed out** past its timeout;
- **ended as cancelled** when an organisation admin cancels it;
- otherwise **reported passed or failed** by the runner.

The runner stops its job when a heartbeat answers stop.

### Ports

Declared in the application's terms (`docs/architecture/structure.md`).

- **Runner registry** (server side): register, remove and list registrations, and find a principal's registrations. Adapters: PostgreSQL (ADR 0013) and in-memory.
- **Job queue** (server side): queue jobs, hand the oldest matching job to a claiming runner, take heartbeats, results and cancellations, and expire leases and timeouts. Adapters: PostgreSQL, where claims never hand one job out twice, and in-memory.
- **Job source** (runner side): the claim, heartbeat and result conversation. Adapters: HTTP, and a test double.
- **Executor driver** (runner side): see `execution.md`.

## Rationale

- The GitHub Actions model, and what would show it wrong: ADR 0009.
- Runner as principal, HTTP long-poll, label routing and the host driver: ADR 0034. The gaon driver, and the result listener that is the runner's only inbound connection: ADR 0046. ADR 0034 supersedes ADR 0009's registration token and runner credentials, ADR 0006's mutual TLS for runners, and the runner identity verification port. Checking an executor's setup without a server: ADR 0047.
- Facts of the GitHub model this design relies on (GitHub documentation):
  - the runner needs only outbound HTTPS on port 443;
  - a job runs on a self-hosted runner that has all the labels it names;
  - runner groups limit which repositories use a runner;
  - an ephemeral runner is de-registered after one job.

## Open questions

- **Pool semantics.** Options: a pool as a named label set; a pool as a first-class entity with an access policy (GitHub's runner groups), possibly shared across organisations. Recommendation: none until a placement need appears that labels within one organisation cannot express (ADR 0034).
- **Runner lifecycle.** Options: ephemeral only; persistent hosts with a disposable environment per job. Recommendation: persistent hosts, since the host's image cache is what ADR 0008 relies on. The host driver gives each job a fresh directory, not an isolated environment.
- **Jobs per runner.** A runner runs one job at a time. Options: keep it; let one runner process run several. Recommendation: keep it until host use shows the need.
