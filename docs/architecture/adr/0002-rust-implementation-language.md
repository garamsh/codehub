# ADR 0002: Rust is the implementation language

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The project is a server, a CLI, an MCP server and a runner. The decision was settled in the design session recorded in issue #1 (item 9). The conventions carry no stack file for it yet.

## Decision

The system is implemented in Rust.

## Consequences

- A stack convention for Rust, `docs/convention/stack-rust.md`, is needed and lands in a separate pull request; until then only the stack-neutral conventions apply.
- The structural rules in `docs/architecture/structure.md` are spelled in Rust's terms by that stack file.

## Signs the decision was wrong

- Contributors or agents cannot deliver changes at the pace the project needs because of the language's build or compile cost.
- A required integration, such as a standards-based authentication library, exists only outside Rust and has to be written or wrapped by the project.
