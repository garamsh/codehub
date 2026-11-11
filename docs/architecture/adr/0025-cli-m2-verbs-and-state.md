# ADR 0025: The M2 CLI has a fixed set of verbs, keeps three items of local state and speaks plain HTTP

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0010 made the `ch` CLI a first-class interface kept to few verbs. Left open in `docs/architecture/interfaces.md` were the verb list, the CLI's local state and its output format for agents.

Alternatives not taken:

- A verb per server operation. It grows with the server and carries over the complexity ADR 0010 avoids.
- An alias or plugin mechanism. Nothing in M2 needs it.
- No local state, so every command names the server, the credential and the repository. It makes each command long for people and agents.
- A local cache of read objects. It is a copy of server data that can go stale.
- JSON by default when stdout is not a terminal. Output then differs by how a command is run.
- A PostgreSQL or S3 driver in `ch`. It would give the CLI storage credentials and knowledge of the store.

## Decision

- The verbs in M2 are `login <server url>`, `use <org>/<repo>`, `put <path> <file>`, `put --delete <path>`, `land`, `log <path>`, `read <path>`, `org create`, `repo create` and `member add|remove`. A draft is named with `--draft <name>`, default `default`. Further verbs remain open.
- Local state is the server URL, the token and the selected `<org>/<repo>`, in `$XDG_CONFIG_HOME/ch/` with mode 0600.
- Output is human text by default and JSON with `--json`.
- `ch` holds no PostgreSQL or S3 driver. It moves object bytes with plain HTTP to presigned URLs.

## Consequences

- The CLI's surface is the list above; a new verb is a decision of its own.
- The token is on disk, readable by the user only; losing the file means logging in again.
- The CLI depends on the HTTP API (ADR 0024) and the presigned URLs of ADR 0023 and nothing else.
- Agents that want structured output pass `--json`.

## Signs the decision was wrong

- Agents repeatedly work around the verb list with scripts that the server could do in one operation.
- Users keep several servers or repositories in use at once, and one selected `<org>/<repo>` forces constant switching.
- The mode 0600 file is not an acceptable place for the token on a platform the CLI must serve.
- Agents omit `--json` and parse text.
