# ADR 0039: Landings of one repository pass a fair gate in each server process, and no limit is set on how many wait

> Status: accepted
> Date: 2026-11-13

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0036 retries a lost compare-and-swap on `main` inside the server. It names serialising landings within one process as measured in #116 and left to the ADR on per-repository limits; this is that ADR. `tenancy.md` left open whether a repository limits its concurrent landings.

The server-side retry alone still lets every landing in flight read `main`, rebase and race for the swap, so under 200 agents most attempts lose. #116 measured three ways of passing landings of one repository through a gate in each process (`docs/scale-baseline.md`):

- with no gate, on main at `595f4c3`, a few landings took most of the lost swaps; 11,295 to 16,104 lost swaps per scenario reached the client, and each such landing took about 150 requests;
- with an unfair async mutex as the gate, lost swaps fell to 0–16, but a landing woken from the queue could be overtaken by one just arriving, again and again: land p99 reached 96–137 s and every scenario overran its 60 s window;
- with a gate that passes landings in arrival order, lost swaps reaching the client fell to 12–25 per scenario and throughput rose 2–6× over main.

With the fair gate, land p50 was 1.9–3.8 s in every scenario and p95 5.5–7.6 s in `disjoint` and `same-file`, near the about 100 landings each process had waiting times a hold of 20–50 ms. Land p99 was 61.8–65.0 s in every scenario, and p95 38.1 s in `read-heavy` and 59.3 s in `external`. That tail is not explained: it was measured with other projects loading the machine (load average 7–34 on 12 cores), and the gate passes landings in arrival order by reading and by its tests. Per-request figures on main hide a longer tail of their own: a landing retried from the client took about 150 requests, about 2 minutes end to end.

Alternatives not taken:

- **No gate.** Measured above: the races, not the work, are the cost.
- **One landing at a time per repository across all replicas**, through a database lock or a landing queue. It would also remove the races between replicas, but holds a PostgreSQL connection or adds a queue for the length of a rebase, and adds a dependency the per-process gate does not need. Two replicas measured 12–25 lost swaps per scenario with per-process gates, which the server-side retry absorbs.
- **A limit on landings waiting at the gate**, answering the excess with 429 or 503. No figure is measured that would set it, and a refused landing is retried by the client, which brings back the round trips ADR 0036 removed.

## Decision

- Each server process passes the landings of one repository through a gate, one at a time, in the order they reach it.
- A landing reaches the gate after authorisation and the draft read, and holds it for its attempts at the swap (ADR 0036).
- A landing that is cancelled while waiting leaves the queue; one cancelled while holding passes the gate to the next.
- A repository's gate exists in a process only while a landing holds it or waits on it.
- Landings of different repositories do not wait on each other.
- An automatic revert (ADR 0035) does not pass the gate; it relies on the retry of ADR 0036.
- No limit is set on how many landings wait. Under a steady load, a landing waits for about the landings ahead of it times the time each holds the gate.

## Consequences

- Lost swaps within one process stop; those between processes remain and are retried by ADR 0036.
- Landing latency grows with the number of landings of one repository in flight in one process, not with how many have lost before it.
- One repository with many agents is bounded by one rebase-and-swap at a time per process. Adding replicas adds parallel gates and brings back races between them.
- Nothing in the HTTP API or the ports changes.

## Signs the decision was wrong

- Land p99 at a fixed agent count, measured on a machine nothing else loads, stays far above the number waiting times the hold time, as the tail of about 60 s did here; that would mean the queue does not drain in order or something inside the hold waits.
- Replicas are added for landing throughput, and lost swaps between them reach the bound of ADR 0036.
- A landing waits long enough for the client or a proxy in front of the server to time it out, which would call for a limit on waiting landings.
