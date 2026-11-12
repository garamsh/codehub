# ADR 0044: Repository changes are delivered at least once from an outbox written in the repository's transaction

> Status: accepted
> Date: 2026-11-13

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0030 reports organisation and repository creation and deletion through the directory-change port. Delivery is at most once: the use case reports a change after it succeeded, and an adapter that fails keeps that to itself (`crates/app/src/port/driven/directory_changes.rs` at `171fb60967154bd45d285cb3f79e5985e2f7449b`). ADR 0030 rejected a durable queue because it needed a store and a retry process before any adapter needed them. `access.md` left reconciliation open until "the first adapter that reports to a real outside layer".

garam is that layer. Its ADR-0149, merged on garam's `dev` branch as `adf2e1c8855dd75b463675f2035c661cd8aad957`, states:
- a repository garam does not hold has no parent organisation, so every decision on it is denied;
- codehub writes `Codehub::Repo` entities through the HTTP form of `ApplyEntityChanges`, with the token of the entity's organisation (ADR 0042);
- an upsert carries the entity `grn:<org>/codehub/repo/<id>`, an `org` attribute holding `<org>`, and its one parent `grn:root/iam/org/<org>`, with no members, consents or operations. A delete is an `EntityDelete` of the same entity;
- each entity carries a sequence that rises with every change. A replayed or reordered delivery changes nothing, so at-least-once delivery is safe;
- organisations are garam's, so codehub reports no organisation change to garam;
- the outbox is stated "for the external mode".

garam's ADR-0149 is "accepted; not built"; its HTTP binding with `ApplyEntityChanges` is garamsh/garam#1809.

A lost repository creation therefore leaves a repository that every request is denied on, and nothing repairs it.

Alternatives not taken:

- **Keep at-most-once delivery and add a periodic reconciliation.** Until the next pass, a repository is denied. The pass needs a listing on both sides and a rule for which side wins.
- **Deliver inside the request and fail the creation when delivery fails.** The repository is then committed in codehub but reported as failed, and garam's availability bounds creating a repository.
- **A message broker.** It is a new service to run, where PostgreSQL, the metadata store, is already there (ADR 0013).

## Decision

- **The outbox.** The repository directory writes each repository creation, and later each deletion, as an outbox entry in the same transaction as the change. It does so in every mode, so one code path serves all of them; with the no-op adapter an entry is accepted on its first delivery. This is codehub's choice where garam's ADR-0149 asks only for the external mode.
  - An entry carries the organisation, the repository's name and id, the kind of change and a sequence.
  - The sequence is a counter on the repository's row, raised in the change's transaction, so it rises with every change to that repository and is never reused. How a deletion keeps the row's counter is decided with deletion.
  - The PostgreSQL adapter keeps the entries in a table, and the in-memory adapter keeps them in memory.
  - A migration writes one creation entry, with sequence 1, for every repository that exists when it runs.
- **The directory outbox port.** A new driven port, implemented by both stores, lets the relay claim one repository's pending entries under a lease, release them, and delete an entry once delivered. A claim holds no row lock while delivering; an expired lease lets another process claim the entries.
- **The relay.** Each server process, and the in-memory composition used by the tests, runs a relay that delivers pending entries through the directory-change port.
  - It claims entries one repository at a time and delivers that repository's entries in sequence order.
  - An entry is deleted once the receiver accepts it. While delivery fails it is retried with a backoff doubling to a capped interval, and every failure is logged with the repository and the sequence.
  - No entry is dropped. An entry the receiver refuses for good stays and is retried at the cap, so its log lines and the outbox's age show it.
  - Delivery is at least once.
- **The directory-change port.** Repository changes and organisation changes become two methods:
  - a repository change carries the organisation, the name, the id and the sequence. The adapter delivers it and says whether it was accepted; an adapter that cannot deliver says so, and the relay retries. The no-op adapter accepts every change;
  - an organisation change is reported by the use case directly, at most once as before (ADR 0030). The in-process adapter, used by the tests, receives both. The garam adapter ignores organisation changes, since garam holds organisations.
- **The garam adapter.** A new adapter delivers to garam's HTTP form of `ApplyEntityChanges`, at the URL in `CODEHUB_DIRECTORY_GARAM_URL`. It sends the entity of ADR-0149 with the entry's sequence, and the token of the entry's organisation, obtained as ADR 0042 decides. Neither adapter calls the other (`structure.md`, "An adapter's only counterpart is its port"): the client of ADR 0042, which obtains and caches one token per organisation, becomes a library both adapters depend on, and the composition root builds one instance of it and hands it to both. A library shared by two adapters is a case `structure.md` does not name. It counts an answer of applied or dropped as accepted.
- **Settings.** `CODEHUB_DIRECTORY_CHANGES` selects `none`, the default, or `garam`. `garam` requires the AuthZEN adapter in the `grn` profile (ADR 0043), and the server refuses to start otherwise.
- **Waiting at creation.** After the creation commits, the request delivers that repository's entries itself, as the relay would, for at most `CODEHUB_DIRECTORY_DELIVERY_WAIT` (default 2 seconds). It then answers 201 whether or not delivery finished, and the relay carries on after the answer.
- This supersedes ADR 0030's delivery "at most once" for repository changes, and ADR 0030's rejection of a durable queue.

## Consequences

- A repository created while garam is unreachable is denied in garam until the relay delivers it. The window is bounded by garam's availability, not by a lost message.
- One more table, one more port, and one background task per server process.
- Nothing of the garam adapter can be shown against garam until garamsh/garam#1809 is served. Until then it is tested against a stand-in that speaks ADR-0149's contract.
- A server stopped between the commit and delivery leaves the entry in the table, and the next process to start delivers it.
- Creating a repository can take up to the configured wait longer.
- The open question "Reconciling directory changes" in `access.md` is settled for repositories by the outbox.

## Signs the decision was wrong

- The outbox's backlog grows faster than the relay drains it, which would show one relay per process is too few.
- Repositories are routinely denied after creation for longer than the configured wait.
- A receiver needs organisation changes from codehub after all.
