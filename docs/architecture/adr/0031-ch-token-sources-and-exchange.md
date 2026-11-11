# ADR 0031: `ch` reads a token from a file or an environment variable and may exchange it by RFC 8693

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0021 gave the CLI the Device Authorization Grant for a person's token, and ADR 0025 keeps the token in `ch`'s local state. With one kind of principal (ADR 0027) and an outside issuer (ADR 0028), a client often already holds a token for another audience, such as a session of the issuer, and must turn it into one for codehub. Goal issue #64 (decision 7) adds that. RFC 8693 is a Proposed Standard (`docs/architecture/access.md`, status table).

Alternatives not taken:

- Writing the exchanged token to `ch`'s state file. A short-lived token would sit on disk past its use.
- Making the server perform the exchange. The server would then hold clients' source tokens.
- Dropping the device grant. Standalone mode has no other login.

## Decision

- `ch` takes a token from a file path or from an environment variable naming the token.
- `ch` may exchange that token by RFC 8693 at a configured token endpoint, for a configured audience, and presents the result to the server.
- The exchanged token is held in memory only.
- The device grant login (RFC 8628) stays for standalone mode.

## Consequences

- An outside layer's token reaches codehub with no copy stored under `ch`'s state directory.
- Each `ch` run that exchanges makes a call to the token endpoint, and its failure fails the command.
- The token endpoint and audience are settings the user or the environment provides.
- ADR 0025's local state of server URL, token and selected `<org>/<repo>` is unchanged for standalone login.

## Signs the decision was wrong

- Exchange latency on every command is noticeable, and users ask for a cache on disk.
- Clients need a source other than a file or an environment variable, such as a credential helper.
- Issuers require client authentication at the token endpoint, which `ch` has no setting for.
