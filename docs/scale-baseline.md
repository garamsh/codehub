# Scale baseline

Throughput and latency of codehub at 200 concurrent agents, measured with `crates/loadtest` (`make loadtest`), and the commit and machine each figure holds for.

## How it is measured

- `make loadtest` builds the release binaries and runs `crates/loadtest` against two `codehub` replicas. The replicas share the PostgreSQL, MinIO, Keycloak and Cerbos services of `compose.yaml`.
- 200 agents drive the HTTP API for 60 seconds per scenario. A landing stages 20 paths.
- The harness answers a 409 whose detail starts with "main moved" by landing the same draft again, and counts it as a lost swap. It answers any other 409 by staging again, and counts it as a conflict.
- There is no fixed target. A figure is compared only with another taken on the same machine with the same harness.

| Scenario | What each agent does |
|---|---|
| `disjoint` | Lands changes to paths no other agent touches |
| `same-file` | Lands changes to its own region of one file shared by every agent |
| `read-heavy` | Mostly reads files and history, and sometimes lands |
| `external` | Lands changes to many paths, with authorisation through AuthZEN (Cerbos) and identity through introspection (Keycloak) |

## Before any improvement

Commit `1cdea4a5f773cedcc53c73c44a8e99a76ffc94d2`, on 12 x AMD Ryzen 5 9600X with 31248 MiB of memory. Pool size 10 per replica, the default at that commit.

| Scenario | Operations/s | Landings | Land p50 / p95 / p99 (ms) | Retries per landing | Lost swaps | Conflicts |
|---|---|---|---|---|---|---|
| `disjoint` | 5.5 | 335 | 421.9 / 4427.2 / 5685.9 | 36.4 | 12187 | 0 |
| `same-file` | 9.1 | 557 | 610.5 / 1552.9 / 2153.5 | 29.1 | 16114 | 103 |
| `read-heavy` | 154.5 | 710 | 464.0 / 2014.4 / 3214.9 | 20.6 | 14604 | 0 |
| `external` | 2.3 | 144 | 717.4 / 2928.6 / 3802.1 | 69.4 | 9996 | 0 |

- Land latency is per landing request, so a landing retried N times counts N + 1 requests.
- `read-heavy` reads: p50 34.6 ms, p95 87.5 ms, p99 261.5 ms. History: p50 49.6 ms, p95 125.0 ms, p99 342.1 ms.
- `same-file` staging: p50 67.8 ms, p95 3017.6 ms, p99 4084.8 ms.
- `external`: 23540 API requests made 263280 decision-point calls, 11.18 per request, and 23540 introspection calls, 1.00 per request.
- In `disjoint`, 62 of the 335 landings were retried, and those 62 took the 12187 lost swaps between them.

## Findings

- **Lost swaps dominate.** Every scenario spends 20 to 70 retries per landing, and nearly every retry is a lost compare-and-swap on `main`, not a conflict. Each one goes back to the client, which pays a round trip, the authorisation of every staged path, a draft read and a rebase. ADR 0036 moves the retry into the server.
- **Authorisation is asked once per path.** In `external` mode a landing of 20 paths asks the decision point 20 times on every attempt. ADR 0038 asks once per request.
- **The connection pool limits reads.** A sweep of the pool size per replica, at 30 seconds per scenario on the same commit and machine, ADR 0037:

| Pool size | `disjoint` operations/s | `disjoint` land p50 (ms) | `read-heavy` operations/s |
|---|---|---|---|
| 4 | 11.6 | 479 | 140.5 |
| 10 | 4.6 | 370 | 165.8 |
| 40 | 19.5 | 257 | 215.6 |

- `disjoint` throughput varies widely between runs at one pool size, because a few landings take most of the lost swaps. `read-heavy` throughput and `disjoint` land p50 move in one direction with the pool size.
