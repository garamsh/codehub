# ADR 0014: The application is partitioned into a domain ring and an application ring

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The decision was settled in the design session recorded in issue #12 (decision 9). ADR 0001 left open whether the inside of the hexagon is partitioned further, and `docs/architecture/structure.md` held the question open until the first domain was written (`docs/architecture/README.md` rule 7). That domain, source control, is now being written: its object model, deltas, landing merge and derived data (issue #12, decisions 1–7).

The reason recorded in that session: merge and rebase logic must be pure functions without I/O, so that tests can prove it end to end with no technology and no port substitute attached. An undivided inside leaves nothing that stops such logic from calling a port.

Alternative not taken:

- The undivided inside of Cockburn's statement (*Hexagonal architecture*, 2005), where the application is one unit and the rule on references inside it is silent.

Issue #12 records no further reason for not taking it.

## Decision

The application is partitioned into two rings, every reference pointing inward: an inner domain ring (the object model, deltas, merge and derivation — pure, with no I/O) and an application ring (use cases and ports). The rules are in `docs/architecture/structure.md`.

## Consequences

- A reference from the domain ring to the application ring, to a port, or to anything outside the application is a defect.
- Domain code has no port to call, so anything it needs from storage or the network is read by a use case and passed in as a value, and anything it produces is returned for a use case to persist.
- Merge, rebase and derivation are testable as functions of their inputs, with no adapter or `crates/adapter-memory` substitute in the test.
- A use case is the only place that sequences I/O around domain logic; a feature touches both rings where it needs both.
- Each new type is placed in one ring or the other, and a type needed by both lives in the domain ring.

## Signs the decision was wrong

- Domain functions take more and more pre-fetched data than they use, because a use case cannot know in advance which objects a merge or a derivation will need — for example a landing that must load every tree on a path before it knows which subtrees differ.
- Lazy or incremental work, such as fetching subtrees during a merge or a derivation that stops early, is reimplemented as a callback or iterator passed into the domain ring that is a port in all but name.
- Use cases grow domain decisions of their own (conflict rules, ref semantics) because the domain ring cannot express them without I/O, and the same rule appears in two use cases.
- The domain ring's tests still need a substitute for something outside, or a defect in merge or landing is found only by a test that runs a use case with adapters.
- The application ring holds only thin wrappers that pass values between a port and one domain function, and no use case sequences more than one step.
