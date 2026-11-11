# ADR 0019: The landing merge merges the file kind apart from content, and a file against a directory conflicts where a delta is applied to a tree

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0017 settled how the landing merge treats a path's content. It did not name two cases the merge meets once objects carry what ADR 0016 encodes:

- **The file kind.** A changeset change, a tree entry and a file-version node record a file as one of two kinds, a file or an executable file; the executable bit is the only file mode (ADR 0016). Either side of a landing can change a path's kind, with or without its content.
- **A file against a directory.** A tree entry names either a file or a subtree, and a tree holds no name twice (`docs/architecture/source-control.md` §Object encoding), so a tree cannot hold a file at `a` and anything under `a/`. The landing merge works path by path over two deltas (ADR 0012), and a file written at `a` on one side and a file written at `a/b` on the other are two different paths, each clean on its own.

On 2026-11-11 the user delegated every choice in issue #19 to the PM ("그냥 너가 알아서 전부다 해줘"). The PM decided both cases under decision 6 of that issue in the review of pull request #27, and issue #28 restates them; this ADR records them.

Alternatives not taken:

- **The kind as part of the content.** Treating a version's kind and content as one value makes a kind change on one side and a content change on the other a conflict, though neither touches what the other changed.
- **One side's kind always wins.** Taking `main`'s kind, or the landing's, drops the other side's change to it without a conflict to say so.
- **File against directory as a per-path merge rule.** The two paths do not overlap, so the per-path merge never compares them; finding the clash there means walking every changed path's ancestors and descendants on both sides, which applying the merged delta to `main`'s tree does in any case, since the tree it builds cannot hold both.
- **Resolving a file against a directory by preferring one side.** Dropping the file, or the paths under it, removes one side's change without a conflict to say so.

## Decision

- **File kind.** A path's kind is merged 3-way, apart from its content. A kind changed on one side only is taken; a kind changed to the same kind on both sides is taken; both sides setting it to different kinds is a conflict on that path.
- **Order of reporting.** Where a path's content and its kind both conflict, the content conflict is the one reported.
- **File against directory.** A delta that leaves a path holding a file while another path lies under it is rejected where the delta is applied to a tree. The landing reports the rejection as a conflict on the path that holds the file. It is not a rule of the per-path merge.

## Consequences

- A landing that makes a script executable merges cleanly with an edit to the same script on `main`, and the result is the edited script, executable.
- A deletion on one side against a kind change on the other is a deletion against a modification, a conflict under ADR 0017.
- With two kinds, a path the base holds cannot conflict on its kind: a side that changed it changed it to the one other kind, so both sides that changed it agree. A kind conflict arises only where both sides add the path with different kinds.
- A kind added later, such as a symbolic link, which ADR 0016 leaves to a new format version, falls under the same rule, and then a path the base holds can conflict on its kind.
- The per-path merge does not see a file against a directory, so a landing whose merge is clean can still be rejected when its result is applied to `main`'s tree. The landing use case reports both kinds of rejection as conflicts on paths, so the author's agent resolves them the same way.

## Signs the decision was wrong

- Kind changes on one side merged beside content changes on the other are followed by post-land check failures or reverts noticeably more often than other clean merges.
- Agents resolving a file-against-directory conflict routinely cannot tell from the reported path which other paths lie under it, and ask for them.
- File-against-directory rejections are a visible share of rejected landings, enough that finding them before tree application would spare agents a round trip.
