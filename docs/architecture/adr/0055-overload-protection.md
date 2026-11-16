# ADR 0055: a process bounds what it takes on and how long it waits

> Status: accepted
> Date: 2026-11-16

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

Until now nothing bounded a process: a caller could send requests as fast as it liked with bodies of any size, a handler that waited on a slow dependency held its request open for ever, any number of requests ran at once, the calls to the identity provider, the decision point and the object store each carried a timeout hard-coded in their adapter or none, and a shutdown waited for every request in flight with no end. Goal #273 puts the server behind a load balancer on Kubernetes, where a pod that does not stop within its grace period (30 seconds) is killed, and where one slow dependency must not take every replica with it. PostgreSQL's pool and statement timeouts are the subject of ADR 0054 and are not repeated here.

## Decision

Each limit is a setting of the roles that serve (`api`, `runner-api`, `all`), with a default that a single process under the checks never meets.

- **Rate.** A token bucket per authenticated principal on the user API: `--rate-limit-per-second` (100) refills it and `--rate-limit-burst` (200) is its size. A request over the limit is answered 429 with `Retry-After`. Authentication comes first, so a request with no valid credential is 401 and does not spend a bucket, and one caller cannot exhaust another's. Buckets that are full again are swept, so the table holds the principals active in the last few seconds and not every principal ever seen.
- **Body size.** `--max-body-size` (1 MiB) bounds what the server reads of a request; more is answered 413. Objects do not pass through the API (ADR 0015), so this bounds JSON and a runner's results, not `--max-object-size`.
- **Request time.** `--request-timeout-seconds` (60) bounds a request; a request that takes longer is dropped and answered 503. Two routes are exempt from this bound alone, because they bound themselves: the runner's claim, which holds for `--runner-claim-hold`, and the landing of a draft, which tries for `--landing-attempts` (ADR 0036). They still count toward the concurrency cap. A test fails if an exempt path is not a route the router has.
- **Concurrency.** `--max-in-flight` (512) bounds the requests being served at once by an atomic counter held for the life of a request; one more is answered 503 with `Retry-After: 1`. The probes `/healthz` and `/readyz` are outside every limit and the layer does not wrap them, so a process at its cap stays ready and is not restarted for being busy.
- **Runner routes.** They are not rate limited: a runner's heartbeat and result are the protocol itself and a refused one costs the job its lease. They are bounded by the concurrency cap, the claim's hold and the heartbeat interval the server sets (ADR 0034). They have an extractor of their own, `AuthenticatedRunner`, that authenticates and does not count.
- **Timeouts of the calls a process makes.** An adapter takes its timeout from its constructor and not from a constant. `--identity-timeout-seconds` (10) covers the key set of OIDC and the endpoint of introspection, which are one provider; `--authzen-timeout-seconds` (10, at most 14) covers the decision point, garam's directory and the token client that serves them; `--s3-connect-timeout-seconds` (3) and `--s3-timeout-seconds` (10) cover the object store. The authzen bound is capped at 14 because one delivery to garam is up to four requests and the relay's lease is 60 seconds (ADR 0044): four of the cap stay inside the lease, so a slow garam cannot hold a row past it. A call that times out is an adapter's server error and is answered 5xx (ADR 0033), never a refusal of the caller.
- **Shutdown.** On SIGTERM or SIGINT the server stops accepting and lets requests in flight finish, for at most `--shutdown-deadline-seconds` (25, the pod's grace of 30 less the time to drain connections); after that it returns and drops them. With nothing in flight it ends at once.
- **Answers.** 413, 429 and 503 are problems of the same shape as every other (ADR 0033); `Retry-After` is a header of the problem and is given where retrying helps (429, the concurrency cap) and not for a timeout, which retrying the same request does not cure. They are added to the description in `openapi.json`.

## Consequences

- The limits are per process. With `n` replicas the rate a principal may sustain is `n` times the setting, and the in-flight cap is per replica. A limit shared across replicas needs a shared store (PostgreSQL on every request, or a cache this project does not run), which #273 does not adopt; the load balancer's own limit is the place for a global one.
- A caller that stays within 100 requests a second is unaffected. A script that bursts past 200 meets 429 and is told when to retry.
- A hung dependency now ends a request in at most the request time, and the connection it held returns to the pool; before, it held both indefinitely.
- Adapter constructors take one more argument; every call site passes it, and the tests pass 10 seconds, which is what the constants were.
- `docs/guide/deployment.md` lists the new options and which roles take them; a test compares that table with each role's `--help`.

## Rejected

- **tower's `limit` feature** (`ConcurrencyLimit`, `RateLimit`). It pulls `tokio-util`, which the lockfile does not have, and it queues a request until a permit is free where the right answer to overload is to refuse, quickly, with a status the caller can act on.
- **504 for a request timeout.** It names a gateway, and this process is not one; 503 is already in the contract, so no status is added.
- **Rate limit by address.** Behind a load balancer every caller shares an address, and the principal is what the policy is about.
- **Rate limit runners too.** A limit on the heartbeat can make a healthy runner lose a job.
- **A timeout on every route, claim and landing included.** The claim's hold is longer than a short timeout by design, and a landing cut off half-way leaves a swap to settle (ADR 0036).
- **A deadline that waits for every request.** It is the behaviour this replaces; a pod is killed at its grace period and the requests are lost without a message, where dropping them at the deadline ends them first and in order.

## Signs the decision was wrong

- Operators run more than one replica and ask for one limit across them, or a principal's legitimate burst is refused at the default.
- A request that is exempt from the timeout hangs, because its own bound does not hold.
- The 25 seconds of shutdown is not enough for the requests that finish, or the pod is killed before it ends.
