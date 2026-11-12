# ADR 0044: Repository changes are delivered at least once from an outbox written in the repository's transaction

> Status: accepted
> Date: 2026-11-13

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0030 reports organisation and repository creation and deletion through the directory-change port. Delivery is at most once: the use case reports a change after it succeeded, and an adapter that fails keeps that to itself (`crates/app/src/port/driven/directory_changes.rs` at `171fb60967154bd45d285cb3f79e5985e2f7449b`). ADR 0030 rejected a durable queue because it needed a store and a retry process before any adapter needed them. `access.md` left reconciliation open until "the first adapter that reports to a real outside layer".

garam is that layer. Its ADR-0149, merged on garam's `dev` branch as `adf2e1c8855dd75b463675f2035c661cd8aad957`, states:
- a repository garam does not hold has no parent organisation, so every decision on it is denied;
- codehub writes `Codehub::Repo` entities through the HTTP form of `ApplyEntityChanges`, with the token of the entity's organisation (ADR 0042);
- an upsert carries the entity `grn:<org>/codehub/repo/<id>` and its one parent `grn:root/iam/org/<org>`. A delete is an `EntityDelete` of the same entity;
- each entity carries a sequence that rises with every change. A replayed or reordered delivery changes nothing, so at-least-once delivery is safe;
- organisations are garam's, so codehub reports no organisation change to garam.

A lost repository creation therefore leaves a repository that every request is denied on, and nothing repairs it.

Alternatives not taken:

- **Keep at-most-once delivery and add a periodic reconciliation.** Until the next pass, a repository is denied. The pass needs a listing on both sides and a rule for which side wins.
- **Deliver inside the request and fail the creation when delivery fails.** The repository is then committed in codehub but reported as failed, and garam's availability bounds creating a repository.
- **A message broker.** It is a new service to run, where PostgreSQL, the metadata store, is already there (ADR 0013).

## Decision

- **The outbox.** The repository directory writes each repository creation, and later each deletion, as an outbox entry in the same transaction as the change.
  - An entry carries the organisation, the repository's name and id, the kind of change and a sequence.
  - The sequence rises with every change to that repository and is never reused.
  - The PostgreSQL adapter keeps the entries in a table, and the in-memory adapter keeps them in memory.
- **The relay.** Each server process runs a relay that delivers pending entries through the directory-change port.
  - It delivers one repository's entries in sequence order.
  - An entry is held by one process at a time.
  - An entry is deleted once the receiver accepts it, and is retried with a bounded backoff while delivery fails.
  - Delivery is at least once.
- **The port.** It changes from "report once" to "deliver this change, and say whether it was accepted". An adapter that cannot deliver says so, and the relay retries. The no-op adapter accepts every change.
- **The garam adapter.** A new adapter of the port delivers to garam's HTTP form of `ApplyEntityChanges`. It sends the entity of ADR-0149 with the entry's sequence and the token of the entry's organisation, obtained as ADR 0042 decides. It counts an answer of applied or dropped as accepted.
- **Organisation changes leave the outbox.** Organisation creation and deletion are not written to it. The in-process adapter, used by the tests, still receives them from the use case directly, at most once as before.
- **Waiting at creation.** Creating a repository waits for its entry's delivery for a bounded time, set by configuration, then answers 201 whether or not delivery finished. The outbox keeps retrying after the answer.
- This supersedes ADR 0030's delivery "at most once" for repository changes.

## Consequences

- A repository created while garam is unreachable is denied in garam until the relay delivers it. The window is bounded by garam's availability, not by a lost message.
- One more table, and one background task per server process.
- A server stopped between the commit and delivery leaves the entry in the table, and the next process to start delivers it.
- Creating a repository can take up to the configured wait longer.
- The open question "Reconciling directory changes" in `access.md` is settled for repositories by the outbox.

## Signs the decision was wrong

- The outbox's backlog grows faster than the relay drains it, which would show one relay per process is too few.
- Repositories are routinely denied after creation for longer than the configured wait.
- A receiver needs organisation changes from codehub after all.
