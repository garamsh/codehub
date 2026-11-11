# ADR 0003: Source control is server-centred and replaces Git

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The decision was settled in the design session recorded in issue #1 (items 1 and 7). The clients of the system are mostly agents, the `ch` CLI, the MCP server and runners, all reaching a server; hundreds of agents are expected to work concurrently (item 2). Git assumes clients that hold a full copy and work offline. The model reference is Anthropic's description of its server-centred source control, which starts from the same observation: clients are almost always online, so the server answers history and blame queries and performs commits, merges and rebases while the client keeps a bounded object cache (https://www.anthropic.com/engineering/redesigning-source-control). That article says its system is not a replacement for Git for general use; this project takes the model for its own, narrower case.

Alternatives not taken:

- Keep Git, with the server holding repositories and agents using clones.
- A Git-compatible backend, speaking Git's wire protocol or storage format over a server-side store.

Issue #1 records that Git is dropped; it states no further reasons for rejecting either alternative.

## Decision

Source control is a server-centred system. Objects are content-addressed and immutable. The changeset is the unit of work. History and blame are derived data computed on the server. Commits and landing are performed on the server. Landing is automated there: a changeset is rebased onto the tip, the tip is advanced by compare-and-swap, checks run after landing, and a failing landing can be reverted. The shape of the system is in `docs/architecture/source-control.md`.

## Consequences

- There is no push: a client sends objects and changeset content to the server, and the server creates changesets.
- History, blame and path history are queries the server answers from derived data; they are not recomputed by clients.
- Git tooling does not work against a workspace. Importing from or exporting to Git is not provided by this decision.
- Landing does not take a lock around the whole workspace; correctness after landing rests on post-land checks and revert, not on a global queue.
- The server is on the path of every operation; a client without the server can do nothing beyond what its cache holds.
- Derived data needs a computation mechanism and storage, and a version of it can become stale or wrong without the objects changing.
- Several details are left open and are listed in `docs/architecture/source-control.md` §Open questions.

## Signs the decision was wrong

- Work that needs a disconnected client appears, such as agents or people working without the server reachable, and the server-only model blocks it.
- A required tool or integration can speak only Git, and the cost of a bridge exceeds the cost a Git-compatible backend would have had.
- Landing without a global lock produces breakage faster than post-land checks and revert repair it.
- History or blame queries become slow or inconsistent as the changeset log grows, because the derived data cannot be kept in step.
- The server is the bottleneck for operations that a client with a full copy would do locally.
