# Runner

How a runner joins the system, receives jobs and runs them: registration, polling, labels, pools and the job lifecycle.

## Current decisions

### Model (ADR 0009)

- Runners follow the GitHub Actions self-hosted runner model.
- A short-lived registration token scoped to an organisation or a repository is exchanged for runner credentials. Runner identity is mTLS, decided in `access.md`.
- A runner polls the server with an outbound long-poll for jobs. The server never connects to a runner.
- Jobs are routed by labels and runner pools.
- One disposable environment runs one job.
- Runners can run outside the server's host.

### Job lifecycle

- A job is queued, assigned to a runner whose labels and pool match, run in a disposable environment through an executor driver, then reported complete with its result. The environment is discarded after the job.
- Images the job needs are fetched as described in `execution.md`.

### Ports

Declared in the application's terms (`docs/architecture/structure.md`).

- **Runner registry** — issues registration tokens, exchanges them for runner credentials, records runners, labels and pools. Adapter: not decided; the metadata database is PostgreSQL (ADR 0013), and whether the runner registry lives in it is open.
- **Job queue** — holds queued jobs, hands one to a matching polling runner, records completion. Adapter: not decided.
- **Job source** — how the runner core receives work: the poll conversation with the server. Adapter on the runner side: the chosen transport, see open questions.
- **Executor driver** — see `execution.md`.

## Rationale

- Why the GitHub Actions model, and what would show it wrong: ADR 0009.
- Facts of that model this design relies on (GitHub documentation): registration tokens expire after one hour; the runner needs only outbound HTTPS on port 443; a job runs on a self-hosted runner that has all the labels it names; runner groups limit which repositories use a runner; an ephemeral runner is de-registered after one job. The long-poll mechanism and default labels were not found in GitHub's documentation and are not claimed here.

## Open questions

- **Runner-to-server transport.** Options: HTTP long-poll, gRPC streaming, WebSocket. Recommendation: HTTP long-poll, the model ADR 0009 follows; revisit if connection count is a problem.
- **Registration token lifetime and reuse.** Options: single-use, one hour reusable (GitHub's documented lifetime). Recommendation: single-use with a short lifetime.
- **Pool semantics.** Options: pool as a named label set, pool as a first-class entity with access policy (GitHub's runner groups). Recommendation: first-class entity scoped to an organisation or repository.
- **Job cancellation and timeouts.** Options: signalled in the poll response, a separate channel. Recommendation: in the poll response, given outbound-only runners.
- **Runner lifecycle.** Options: ephemeral only, persistent hosts with a disposable environment per job. Recommendation: persistent hosts, since the host's image cache is what ADR 0008 relies on.
