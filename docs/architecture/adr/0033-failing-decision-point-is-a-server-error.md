# ADR 0033: A failing decision point fails the request as a server error and allows nothing

> Status: accepted
> Date: 2026-11-12

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0030 says that on an unreachable or failing decision point "the decision is deny". As built, the AuthZEN adapter returns a backend error. The API answers it 500, as it answers every port's backend failure (ADR 0024), and the request does nothing. The architecture review in goal issue #91 (finding D2) found that the wording and the behaviour disagree.

Alternatives not taken:

- Answering 403 or 404, as a real denial is answered. The caller would read an outage as a lack of permission, and would retry differently, ask for a grant, or give up, when the cause is the decision point.
- Answering 503. ADR 0024 maps every backend failure to 500, and singling out one port would make the status depend on which dependency failed.

## Decision

- An unreachable or failing decision point fails the request as a server error (500 under ADR 0024), and the request has no effect. Nothing is allowed on failure, and no decision is cached.
- This supersedes the clause of ADR 0030 that calls the failure a deny. The rest of ADR 0030 stands.

## Consequences

- The fail-closed property of ADR 0030 is unchanged: a failure never allows.
- A caller can tell an outage of the decision point (500) from a denial (403, or 404 where existence is hidden).
- The behaviour on `main` already matches; no code changes.

## Signs the decision was wrong

- Callers retry 500s aggressively and the retries overload a decision point that is already failing.
- An operator needs to tell decision-point failures from other server errors, and the shared 500 hides which it was.
