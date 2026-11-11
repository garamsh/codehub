# ADR 0012: A workspace has named refs moved by compare-and-swap, and landing merges at line level

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0003 fixed the landing order (rebase onto the current tip, advance it by compare-and-swap, run checks after landing, revert on failure) and left open what the compare-and-swap compares and what happens when the rebase conflicts. ADR 0011 made a changeset a delta against its first parent. The user confirmed refs and the landing merge in the design session recorded in issue #12 (decisions 6 and 7).

What Anthropic's account of Scion (https://www.anthropic.com/engineering/redesigning-source-control) states and this ADR takes:

- Scion discourages refs except the main ref and release branches.
- Moving a ref is a compare-and-swap on a single row, separate from creating changesets.
- Concurrent changes can conflict, and Claude typically fixes those conflicts.
- Automated testing runs after merge rather than behind a global lock.

Reasons recorded from the same session in the PM's comment on issue #12 (https://github.com/garamsh/codehub/issues/12#issuecomment-6792743074):

- With hundreds of concurrent agents, rejecting non-overlapping edits to the same file would cause retry storms, so line-level 3-way merge decides landing throughput; re-parenting with no merge when paths do not overlap keeps landing cost proportional to the change.
- A broken `main` contaminates work based on it until the revert. The `last-green` ref answers that; it is the PM's proposal and does not come from the Scion article.

Alternatives not taken:

- Work branches, one named ref per piece of pending work.
- Path-level merge for text files, rejecting any landing whose changed files overlap the changes since its base. Rejected for the retry storms above.
- A merge changeset for every landing, including landings whose paths do not overlap. Rejected for the landing cost above.
- CI and deployment basing on `main`. Rejected for the contamination above.
- Resolving true conflicts on the server by a per-path policy.

Issue #12 states no reason against the first and the last beyond the decisions themselves.

## Decision

- **Refs.** A workspace has named refs: `main`, release refs, and `last-green`. Agents base work on `main`; CI and deployment base on `last-green`. There are no work branches: pending work is a server-side draft changeset outside the log. Moving a ref is a compare-and-swap on that ref alone, comparing the ref's current changeset identifier, and is separate from creating the changeset. Only `main` is implemented first; the format admits more refs.
- **Landing merge.** A changeset whose paths do not overlap the changes made since its base is re-parented onto `main` with no merge. Overlapping text files are merged by a line-level 3-way merge; overlapping binary files are merged at path level. A true conflict rejects the landing and returns the conflicting paths for the author's agent to resolve. The landing order of ADR 0003 stands.

The resulting shape is in `docs/architecture/source-control.md`.

## Consequences

- Concurrent landings that touch different lines of one text file both land without a retry.
- The cost of landing a changeset whose paths do not overlap is creating one changeset with a new parent; it does not grow with the number of landings since its base.
- A clean line-level merge can still produce code that does not build or pass; post-land checks and revert (ADR 0003) are what catch it.
- Two landings that edit the same binary file conflict, whatever bytes each changed.
- A changeset can be created and then lose the compare-and-swap on `main`; it then exists without a ref pointing at it, and the landing is not applied (ADR 0003).
- No ref names pending work, so sharing unlanded work between agents goes through draft changesets, not branches.
- Until `last-green` is implemented, CI and deployment have only `main` to base on.
- What moves `last-green`, how release refs are created, and the details of the merge are left open in `docs/architecture/source-control.md` §Open questions.

## Signs the decision was wrong

- Landings reverted after a clean line-level merge are measurably more frequent than landings reverted after a re-parent, and the breakage they cause outweighs the retries a path-level rule would have caused.
- Agents still retry the same landing repeatedly on frequently edited files, because true conflicts on those files are common enough that line-level merge does not prevent retry storms.
- The compare-and-swap on `main` fails for a large share of landings under concurrent load, so throughput is bounded by the single ref rather than by the merge.
- `last-green` lags `main` by so many changesets that CI and deployment run on code agents left behind long ago.
- Agents need named, long-lived pending work that several of them build on, and draft changesets cannot carry it without becoming branches in all but name.
- Changesets created for landings that lost the compare-and-swap accumulate faster than retention can remove them.
- Files classified as binary conflict at path level often enough that landings touching them are rejected markedly more often than landings touching only text files.
- Authors' agents cannot resolve the returned conflicting paths without more context than the paths, and a server-side resolution would have been cheaper.
