# ADR 0017: The landing merge diffs lines by histogram, tells binary by content, and detects no renames

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0012 decided that overlapping text files are merged by a line-level 3-way merge, that overlapping binary files are merged at path level, and that a true conflict rejects the landing. It left open which line diff the merge uses, how a file is classified as text or binary, which overlaps are true conflicts, and whether renames are detected. Milestone M1 implements the landing merge in the domain ring (ADR 0014), so these have to be settled before it is written. On 2026-11-11 the user delegated every choice in issue #19 to the PM ("그냥 너가 알아서 전부다 해줘"); this ADR records decision 6 of that issue.

Facts the decision rests on:

- The `imara-diff` crate, release 0.2.0 (https://crates.io/crates/imara-diff/0.2.0), offers two algorithms: a linear-space Myers variant and Histogram. Its README states that the histogram algorithm "is a variant of the patience diff algorithm", "was originally ported from git but has been heavily optimized", and "outperforms Myers algorithm by 10% - 100% across a wide variety of workloads". It computes a 2-way diff only; it has no 3-way merge.
- Git classifies a buffer as binary when a NUL byte occurs in its first 8000 bytes (`FIRST_FEW_BYTES` in `xdiff-interface.c`, git commit 1891de46f79dc444a47db1c87bd0c8c76b5463f2).

Alternatives not taken:

- **Myers diff.** Histogram is a patience variant: it anchors on lines that occur rarely, so a change is less often aligned against unrelated common lines such as lone braces. A misaligned diff in a 3-way merge turns a clean merge into a spurious conflict, or splits one edit across two hunks. `imara-diff` also reports Histogram as the faster of its two.
- **Rename detection in M1.** Pairing a deleted and an added path by content similarity needs a similarity threshold and a pairing rule, and a wrong pairing merges edits into the wrong file. Without it, a rename on one side and an edit on the other is a delete against a modify, which is a conflict the author's agent resolves.
- **Whitespace-insensitive merge.** Ignoring whitespace when matching lines leaves the merge to choose whose whitespace survives, which silently drops one side's change; in languages where indentation is syntax it changes meaning.
- **A per-path text or binary attribute.** Classifying by content needs no configuration; an attribute can be added later without changing the content rule for paths that carry none.

## Decision

- **Line diff.** The landing merge diffs lines with the Histogram algorithm of the `imara-diff` crate. The 3-way merge over those diffs is written in the domain ring; `imara-diff` supplies only the 2-way diff.
- **Text or binary.** A file version is binary when a NUL byte occurs in its first 8000 bytes or its content is not valid UTF-8; otherwise it is text. A path is merged by lines only when its base and both sides are text; otherwise it is merged at path level.
- **Whitespace.** Lines are compared byte for byte. A whitespace-only difference, a line-ending difference included, is a difference.
- **Renames.** None are detected. A rename is a deletion of one path and an addition of another.
- **True conflicts.**
  - one side deletes a path and the other modifies it;
  - both sides add a path with different content;
  - both sides change the same lines of a text file differently: their changed ranges of base lines overlap, or both insert at the same base position;
  - both sides change a binary path to different content.
- **Clean.** Identical changes on both sides, a path deleted on both sides, and changes to disjoint line ranges of one text file merge without conflict.

The resulting shape is in `docs/architecture/source-control.md`.

## Consequences

- A rename on one side with an edit on the other is rejected as a conflict, and the author's agent re-applies the edit at the new path.
- A reformatting change on one side conflicts with any edit to the reformatted lines on the other.
- A text file in an encoding other than UTF-8, such as UTF-16 or Latin-1, is merged at path level.
- A file whose first NUL byte comes after its first 8000 bytes and which is valid UTF-8 is treated as text.
- The project depends on `imara-diff`, verified under `docs/convention/ci.md` §Verify a pinned dependency before adopting it when it is added; replacing it touches only the 2-way diff, since the 3-way merge is the project's own code.
- Changes on adjacent but not overlapping line ranges merge cleanly, where git reports them as a conflict: git's `xdl_do_merge` (`xdiff/xmerge.c`, git commit 1891de46f79dc444a47db1c87bd0c8c76b5463f2) separates two changes only when the first ends strictly before the second begins.

## Signs the decision was wrong

- Landings rejected for a delete-against-modify conflict are, on inspection, mostly renames, often enough that agents spend more on re-applying edits than rename detection would have cost.
- Conflicts reported on overlapping line ranges are, on inspection, often misalignments that a Myers diff of the same inputs merges cleanly.
- Clean merges of adjacent line ranges are followed by post-land check failures or reverts noticeably more often than other clean merges.
- Landings conflict on whitespace-only or line-ending differences often enough that agents routinely normalise files before landing.
- Text files in encodings other than UTF-8 are common in workspaces, and their path-level conflicts are a visible share of rejected landings.
- Files that are binary but pass the text test, or text but fail it, are merged the wrong way often enough to be reported.
