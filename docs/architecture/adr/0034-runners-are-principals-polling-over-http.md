# ADR 0034: A runner is a registered principal that long-polls the HTTP API and runs jobs on its host

> Status: accepted
> Date: 2026-11-12

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0009 follows the GitHub Actions self-hosted runner model, in which a short-lived registration token is exchanged for runner credentials and runner identity is carried by mutual TLS (ADR 0006). Since then, ADR 0027 made every caller one kind of principal whose token comes from an outside issuer, and left runners to "the runner design". ADR 0029 and ADR 0030 let that issuer and a decision point be outside codehub. A registration token and a certificate authority would make codehub issue credentials, which ADR 0006 says the core does not do.

Goal issue #91 (decision 4) settled the M4 runner with the user:
- a runner is an ordinary principal, registered by an organisation admin, with no registration token, no mTLS and no certificate authority;
- the transport is HTTP long-poll, with cancellation and timeouts in the poll response;
- the host executor is the only executor;
- routing is by labels within an organisation or a repository.

Alternatives not taken:
- **A registration token exchanged for a runner credential (ADR 0009).** codehub would issue and rotate credentials of its own.
- **mTLS with a project CA (ADR 0006, ADR 0009).** It needs a CA to operate and a second listener, and a second kind of identity that ADR 0027 removed for agents.
- **gRPC streaming or WebSocket.** Either is a second transport beside the HTTP JSON API (ADR 0015).
- **A first-class pool entity.** No placement need is known yet that labels within a scope cannot express.

## Decision

**Identity**
- A runner is a principal (ADR 0027). It calls with a bearer token from the configured issuer, verified like any other caller (ADR 0029). How it obtains the token, for example by OAuth client credentials, is the issuer's and the runner's concern.
- An organisation admin registers a principal (issuer and subject) as a runner of the organisation, or of one repository in it, with a set of labels. Removing and listing registrations take the same permission: a new action, administer runners, asked of the organisation through the authorisation decision port. In external mode, the decision point decides it.
- The runner registry holds the registrations. It is the operational record of which principal takes which jobs, so it is kept in both authorisation modes. A principal with no registration that polls is refused.

**What this supersedes**
- It supersedes:
  - ADR 0009's registration token and runner credentials;
  - ADR 0006's mutual TLS for runner identity;
  - the runner identity verification port of `access.md`.
- The rest of ADR 0009 and ADR 0006 stands.

**Transport**
- The runner calls the same HTTP JSON API as `ch`, under `/v1/runner/`, and only outbound (ADR 0009).
- It asks for a job with a long-poll that the server holds for a configured time and answers with a job or with none.
- While running a job it reports a heartbeat. The heartbeat's answer says whether to continue, or to stop because the job was cancelled or timed out. It ends the job by reporting the result.

**Routing**
- A job carries its organisation, its repository and a set of labels. It is offered only to a runner registered for that organisation, or for that repository, whose labels include every label of the job.

**Leases**
- An assigned job whose runner sends no heartbeat within a lease returns to the queue.
- A job still running past its timeout ends as timed out.
- A job cancelled while queued or running ends as cancelled.

**Executor**
- The runner core calls the executor driver port (ADR 0008), and the only driver built is the host driver.
- The host driver runs a job in a fresh directory on the runner's host, with the job's command under `/bin/sh -c`. It stops the job's process group on cancel or timeout, and removes the directory afterwards.
- The job gives the source as presigned download URLs for the files of the changeset (ADR 0015), so the runner needs no `read` grant on the repository.

**Program**
- The runner is a separate binary, `codehub-runner`, in its own package beside `ch`. It holds the runner core, the job-source and executor-driver ports, and their adapters.

## Consequences

- Runners need no credential that codehub issues. The issuer that serves people and agents serves runners, and revoking the principal at the issuer stops the runner (at once in introspection mode).
- A runner reaches the API it already speaks to, with no second listener. `interfaces.md`'s open question on a separate runner endpoint is settled.
- The host driver isolates nothing. A job runs with the runner process's user and host. Until an isolating driver exists (microvm or kubernetes, ADR 0008), a runner must be trusted with what its jobs run.
- The server holds registrations, queued jobs and leases that survive restarts.
- Pools and a first-class runner group are not built. Labels within an organisation or a repository are the only routing.

## Signs the decision was wrong

- Issuers in use cannot give a runner a token without a person present, so registration needs a codehub-issued credential after all.
- Long-poll connections from hundreds of runners exhaust the server's connections or the database's.
- Placement needs, such as a cache-holding host or a shared runner across organisations, cannot be expressed with labels inside one organisation.
- Jobs on the host driver leave state that breaks later jobs, and an isolating driver is needed before checks can be trusted.
