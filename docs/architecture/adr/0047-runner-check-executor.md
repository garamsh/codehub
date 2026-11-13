# ADR 0047: `codehub-runner check-executor` runs one job with a given command through the configured executor, without a server

> Status: accepted
> Date: 2026-11-13

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

A runner takes its jobs from the server (ADR 0034), and the server needs PostgreSQL, an object store and an identity issuer (`docs/deployment.md` §What the server needs). An operator who sets up a runner's executor, such as the gaon driver of ADR 0046 with its ServiceAccount, its RBAC, its result listener and its egress, learns whether that setup works only when a check of a real repository is claimed. When a job fails there, the cause may be the setup or the check.

garamsh/aurora#749 accepts codehub's criterion "a driver that runs each job in a MicroVM on gaon" only on output of a job running in a booted MicroVM on aurora's cluster (aurora#749 issuecomment-6846613539, decision 11). No codehub server runs on that cluster yet. aurora declares one once gaon serves Service, the gateway and Secret through `gaon apply` (garamsh/gaon#207) on a deployed naru release (aurora#750 issuecomment-6840854355), and the release that serves them comes after the one that first serves MicroVMs (aurora#750). The workstation cannot stand in for the server: aurora refuses a path from job MicroVMs back to it (aurora#750 issuecomment-6848796892).

Alternatives not taken:

- **Deploy the server, its database, an object store and an issuer on the cluster first**, either as aurora's deployment of codehub or as a short-lived set codehub runs in MicroVMs beside the runner. aurora's deployment waits on the release above. A short-lived set is four more services codehub would declare, configure and reach by address, with an issuer the server accepts, all to hand out one job. Either ties the driver's evidence to every other piece being up, and a failure would not say which piece failed.
- **A stand-in server that hands out one job.** It would be a second implementation of the job protocol, kept in step with the real one by nothing, and it would need its own image.
- **A test binary run on the cluster.** codehub publishes two images, and a third for one run is a release artefact that nothing else uses.

## Decision

- `codehub-runner check-executor --command <command>` runs one job through the executor port and the driver that `--executor` and its settings configure:
  - the job has no files and the given command;
  - its identifier is drawn at random for each run, so two checks do not share object names;
  - it is stopped as timed out after `--timeout <seconds>`, 600 by default. No server gives it a timeout, so the mode keeps the timer itself.
- It prints to standard output one line naming the job's end (the exit code, a signal, or the stop and its reason), then the output tail the driver returned. It exits with the command's exit code where the command exited with a code from 0 to 255, and with 1 for any other end, as `run-job` does. An executor error is printed to standard error and exits 125, which no end of the command gives.
- It contacts no codehub server and needs no runner credential, so `--server` and the token settings are neither required nor read. The executor's own settings are required as they are for a runner, and are refused at start in the same way.
- With the gaon driver it cleans its runner name's leftovers at start, as a runner does (ADR 0046). Its `--gaon-runner-name` is therefore one of its own and never a live runner's, whose in-flight job it would delete.
- It is a mode for an operator checking a runner's setup. It is not a way to run work: it reports nothing to a server and nothing records it.

## Consequences

- One job runs on gaon with the runner's real code path. That path covers:
  - the Secret and the MicroVM with their label;
  - the job's own `run-job` inside the MicroVM, which posts its result;
  - the phase read and the deletions;
  - the cleanup at start.

  The server's claim and heartbeat are not on that path. They are the same for every executor, and the host-driver tests and the end-to-end suite show them.
- A job with no files fetches nothing, so the check needs no object store. Egress to an object store is therefore not checked by it.
- `docs/deployment.md` can tell an operator to run it before registering a runner. The mode's options belong to the subcommand, not to the runner's options table.

## Signs the decision was wrong

- A setup that `check-executor` passes fails on the first claimed job for a reason of the executor's setup, not of the check.
- The mode is used to run work, not to check a setup.
