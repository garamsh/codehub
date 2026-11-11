# ADR 0009: Runners follow the GitHub Actions self-hosted runner model

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

Runners must run outside the server's host, behind firewalls, and execute untrusted code. The design session recorded in issue #1 (item 6) chose to follow the GitHub Actions self-hosted runner model.

## Decision

- A short-lived registration token scoped to an organisation or a workspace is exchanged for runner credentials. Runner identity afterwards is carried by mTLS, as recorded in ADR 0006.
- A runner obtains jobs by an outbound long-poll to the server; the server opens no connection to a runner.
- Jobs are routed by labels and runner pools.
- One disposable environment runs one job.
- Runners can run outside the server's host.

## Consequences

- Runners need only outbound network access.
- A job waits when no runner with all its labels is available; queueing and its limits are server behaviour to specify.
- Disposable environments bound what one job can leave behind for the next.
- The server holds registration, pools and job state that must survive restarts.

## Signs the decision was wrong

- Long-poll latency or connection count prevents the target of hundreds of concurrent agents.
- Runners must be reached from the server, for example for cancellation, and outbound-only forces workarounds.
- Label routing cannot express the placement jobs need, such as image locality.
- Creating a disposable environment per job costs more than jobs take to run.
