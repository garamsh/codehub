# ADR 0036: The server retries a landing that lost the compare-and-swap on `main`

> Status: accepted
> Date: 2026-11-12

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0012 moves `main` by compare-and-swap, and expects two concurrent landings that change different paths to both land without a retry. As built, a landing that loses the swap answers 409 "main moved while landing; land again", and the client lands the same draft again.

The scale baseline (`docs/scale-baseline.md`, commit `1cdea4a5f773cedcc53c73c44a8e99a76ffc94d2`) measured 20 to 70 retries per landing at 200 agents against two replicas, nearly all of them lost swaps. Each retry from the client repeats a round trip, the authorisation of every staged path, the draft read and the rebase.

Alternatives not taken:

- **Leave the retry to the client.** That is the measured behaviour, and the cost of each retry is the cost of a whole request.
- **One landing at a time per repository**, across all replicas, through a database lock or a landing queue. It removes lost swaps, but serialises landings that do not overlap and adds a wait that grows with the number of agents. Serialising within one process is measured in #116 and decided by the ADR on per-repository limits.
- **Retry without a bound.** A landing that keeps losing would hold its request open for as long as other landings keep winning.

## Decision

- A landing that loses the compare-and-swap on `main` is retried inside the server: it reads `main` again, rebases the same staged delta onto it, and tries the swap again.
- Authorisation and the draft read happen once, before the first attempt.
- A conflict found on a later attempt ends the landing with the conflict, as on the first.
- The attempts are bounded by a setting of the server, 32 by default. When the bound is spent, the answer is the 409 it is today.
- The HTTP API does not change.

## Consequences

- A landing is answered 409 "main moved" only after the bound is spent, so a client's retry is the exception.
- One landing request can take up to the bound's number of rebases, so its latency rises while the number of requests falls.
- An automatic revert (ADR 0035) is retried the same way.

## Signs the decision was wrong

- Landing latency at p99 rises above what the baseline measured for the same throughput, because a few landings spend the whole bound.
- Landings still reach the bound under load, and the 409 rate at the client does not fall.
