# ADR 0041: The AuthZEN adapter sends Access Evaluations only when the operator turns it on

> Status: accepted
> Date: 2026-11-13

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0038 has the AuthZEN adapter send one Access Evaluations request wherever the decision point advertises `access_evaluations_endpoint`. ADR 0030 maps a repository to a resource of type `repository` whose id is `<organisation>/<name>`, and carries the path as the resource property `path`. All the paths of one landing therefore share one resource type and id.

While implementing ADR 0038 (#119), the worker found that Cerbos 0.57.0, the development decision point, answers such a batch wrongly. The PM reproduced it on 2026-11-13. Cerbos treats evaluations with the same resource type and id as one, and the first evaluation's decision answers all of them:
- for bob, who may write under `docs/` in `acme/app`, writing `[docs/x, src/main.rs]` was answered `[true, true]` in one batch;
- the same two paths asked one by one were answered `[true, false]`.

That is a path allowed that the policy denies. Nothing in the metadata tells such a decision point from one that decides each evaluation by its own properties.

Alternatives not taken:

- **Keep ADR 0038 as it is.** Any deployment whose decision point behaves as Cerbos does allows paths its policy denies. That breaks ADR 0033's rule that a failing decision point allows nothing, in a way that does not show as a failure.
- **Give each path its own resource id**, such as `<organisation>/<name>/<path>`. Distinct ids keep the batch correct on Cerbos. But they change the resource every external decision point is written against (ADR 0030), and a federated layer's policies with it (ADR 0028), to work around one decision point.
- **Give up the batch.** The port's one call for many paths still lets a decision point that decides correctly answer a landing in one request. Turning the batch on is the operator's statement that theirs does.

## Decision

- The AuthZEN adapter sends Access Evaluations only when the server's setting `CODEHUB_AUTHZEN_EVALUATIONS` is on and the metadata advertises `access_evaluations_endpoint`. The setting is off by default.
- Otherwise the adapter sends one Access Evaluation per resource.
- The rest of ADR 0038 stands:
  - the port decides many resources in one call;
  - a request with many paths makes that call once;
  - failures and a mismatched count fail the request and allow nothing (ADR 0033).
- This supersedes ADR 0038's bullet on when the adapter sends Access Evaluations.

## Consequences

- By default, decision-point calls per landing still grow with the number of paths in external mode, as before ADR 0038.
- An operator who turns the setting on vouches that their decision point decides each evaluation by its own resource properties. `access.md` names the Cerbos behaviour as the reason it is off.
- The development stack runs with the setting off.

## Signs the decision was wrong

- Decision points that decide each evaluation by its own properties are the common case, and operators leave a correct batch off.
- A decision point collapses evaluations on something other than the resource id, so the setting is turned on where it should not be.
