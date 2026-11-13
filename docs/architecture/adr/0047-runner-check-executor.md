# ADR 0047: `codehub-runner check-executor` runs one job with a given command through the configured executor, without a server

> Status: accepted
> Date: 2026-11-13

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

A runner takes its jobs from the server (ADR 0034), and the server needs PostgreSQL, an object store and an identity issuer (`docs/deployment.md` §What the server needs). An operator who sets up a runner's executor, such as the gaon driver of ADR 0046 with its ServiceAccount, its RBAC, its result listener and its egress, learns whether that setup works only when a check of a real repository is claimed. When a job fails there, the cause may be the setup or the check.

garamsh/aurora#749 accepts codehub's criterion "a driver that runs each job in a MicroVM on gaon" only on output of a job running in a booted MicroVM on aurora's cluster (aurora#749 issuecomment-6846613539, decision 11). No codehub server runs on that cluster yet: its database, bucket and name wait on gaon's Service, gateway and Secret kinds being deployed (aurora#750). The workstation cannot stand in for the server. It is not on the Nodes' network, and a path from job MicroVMs back to it is refused (aurora#750 issuecomment-6848796892).

Alternatives not taken:

- **Deploy the server, its database, an object store and an issuer on the cluster first.** That is aurora's deployment of codehub, which the Function rehearsal of aurora#749 shows anyway. It would tie the driver's evidence to every other piece being up, and a failure would not say which piece failed.
- **A stand-in server that hands out one job.** It would be a second implementation of the job protocol, kept in step with the real one by nothing, and it would need its own image.
- **A test binary run on the cluster.** codehub publishes two images, and a third for one run is a release artefact that nothing else uses.

## Decision

- `codehub-runner check-executor --command <command>` runs one job through the executor that `--executor` and its settings configure, exactly as a claimed job runs:
  - the job has no files and the given command;
  - it has the runner's default timeout, or `--timeout <seconds>`;
  - its identifier is fixed and its organisation and repository are empty.
- It prints the job's end and output tail to standard output, as the runner prints a job's end, and exits with the command's exit code. An executor error is printed to standard error and exits 1.
- It contacts no server and needs no runner credential, so `--server` and the token settings are neither required nor read. The executor's own settings are required as they are for a runner, and are refused at start in the same way.
- It is a mode for an operator checking a runner's setup. It is not a way to run work: it reports nothing to a server and nothing records it.

## Consequences

- One job runs on gaon with the runner's real code path. That path covers:
  - the Secret and the MicroVM with their label;
  - the job's own `run-job` inside the MicroVM, which posts its result;
  - the phase read and the deletions;
  - the cleanup at start.

  The server's claim and heartbeat are not on that path. They are the same for every executor, and the host-driver tests and the end-to-end suite show them.
- A job with no files fetches nothing, so the check needs no object store. Egress to an object store is therefore not checked by it.
- `docs/deployment.md` can tell an operator to run it before registering a runner.

## Signs the decision was wrong

- A setup that `check-executor` passes fails on the first claimed job for a reason of the executor's setup, not of the check.
- The mode is used to run work, not to check a setup.
