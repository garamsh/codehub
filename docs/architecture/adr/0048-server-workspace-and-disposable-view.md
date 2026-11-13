# ADR 0048: A draft is the caller's server-side workspace, opened as a disposable view and snapshotted by each `ch` command

> Status: accepted
> Date: 2026-11-14

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The user decided the direction on 2026-11-14, as recorded in issue #175. The decision is to work on a repository through a workspace held on the server. The agent's side holds only a view of that workspace, and staging does not exist. The PM was granted the details.

What existed before:

- The only way to change a repository was `ch put <path> <file>`, one request per file into a draft, followed by `ch land` (ADR 0025).
- The API had no way to:
  - list a changeset's files;
  - read a draft back;
  - see what a draft changes against its base;
  - undo one staged path.
- An agent that wants to build or test needs the repository's files on a filesystem. An agent that lost its context could not find out from the server what its draft held.

"Workspace" here names a draft. ADR 0004 and ADR 0012 used the word for what ADR 0020 renamed a repository; that meaning is retired, so the word is free.

The models taken:

- **Google's CitC:** the workspace is held centrally and the developer's side is a view of it.
- **Jujutsu:** the working copy is the change itself. A command snapshots it, so there is no staging step and no second state between "edited" and "recorded".

Alternatives not taken:

- **A lazy filesystem view (FUSE-like) now.** It is deferred to a later goal behind the same interface. It costs a platform dependency and a daemon before any repository is large enough to need it.
- **A background daemon syncing on file events.** It adds a process an agent must manage. A snapshot on each `ch` command records the work at every point the agent asks the system anything.
- **No view, with builds run remotely as jobs.** Every edit-and-test loop becomes a remote round trip.
- **Line-level staging, or landing part of a workspace.** Partial work goes in a separate workspace instead. A second recorded state is what makes an agent lose track of what it changed.
- **Diffs computed by `ch`.** That would put a workflow step in the client. ADR 0010 keeps complex work on the server, and the server already diffs lines for the landing merge (ADR 0017).

## Decision

- **A draft is a workspace.** It keeps its identity (repository, principal, name; ADR 0022) and its base. It gains a **description**, initially empty.
  - Opening a workspace fixes its base at `main` as it stands then. In a repository where `main` is unset, the workspace has no base, and its view starts empty.
  - A workspace closes when it lands, as a draft is removed today.
  - One workspace serves one task. Following `main` within a task is not provided; the server's rebase at landing handles a moved `main` (ADR 0012).
- **The API gains, all under `/v1/orgs/{org}/repos/{repo}`:**
  - `PUT drafts/{draft}` — opens the caller's workspace. It creates the workspace on `main` where none exists and answers the workspace either way, so reopening is the same request.
  - `GET drafts/{draft}` — the workspace:
    - its base and description;
    - each changed path with its status (added, modified or deleted against the base), its kind, and a grant to download the content of an added or modified path.
  - `PUT drafts/{draft}/description` — sets the description.
  - `POST drafts/{draft}/snapshot` — records a set of path changes in one request, applied together or not at all. Each change is one of:
    - a file's kind and content address;
    - a deletion;
    - a restore, which removes the path's change so the path is as the base holds it.
  - `DELETE drafts/{draft}/changes/{*path}` — restores one path.
  - `GET drafts/{draft}/diff`, with an optional `path` query parameter limiting it to one path — the workspace's changes against its base, computed by the server:
    - a line diff for each text path;
    - a marker for each binary path or each path whose kind changed.
  - `GET changesets/{id}/files` — every file in the changeset's tree, optionally under a path prefix. Each comes with its path, kind, content address and a download grant.
    - Read is asked for every listed path in one decision, as a landing asks write for all its paths (ADR 0038). A caller holding no `read` grant for any one of them is refused the whole listing, so a listing never shows a path its caller may not read.
    - This decides nothing about a directory listing that omits or shows such paths: that stays the open question in `access.md`. A caller whose grants are narrower than the repository opens a view under a prefix it may read.
  - `POST drafts/{draft}/land` — accepts an empty or absent message and then lands with the workspace's description.
- **`ch` gains these verbs:**
  - `open <dir> [--draft <name>] [--prefix <path>]` — opens the workspace and writes the view into `<dir>`, a directory that is empty or absent. With `--prefix`, the view holds only the files under that path, and a snapshot records only paths under it. A view is the base's files with the workspace's changes applied, so reopening an existing workspace in a new directory restores the work.
  - `status` — the workspace's changed paths, as `GET drafts/{draft}` answers them.
  - `diff [<path>]` — the server's diff.
  - `describe <message>` — sets the description.
  - `restore <path>` — restores the path on the server and rewrites it in the view from the base. An added path is removed.
  - `land`, run in a view, lands the workspace and marks the view closed. There `--message` is optional, and the description is used when it is absent; outside a view `land` still requires it. A later command in that view is refused and names `ch open`.
  - **Snapshots:**
    - Every `ch` command run in a view first snapshots it: `status`, `diff`, `describe`, `restore` and `land`. A command not in this list does not.
    - The snapshot compares the view's files against what the view last recorded, uploads changed content by presigned URL, and sends one snapshot request.
    - A file returned to its base content is sent as a restore.
- **A view keeps its own state** in a `.ch/` directory at its root. The state is the repository, the draft name, the base, the open or closed state, and what each path held at the last snapshot. `.ch/` is never snapshotted.
- **Ignore rules:**
  - They live in `.codehub/ignore` in the repository, written in gitignore syntax.
  - A path they match is not snapshotted unless the base already holds it.
  - The view reads the file as it stands on disk, so an edit to it applies at the next snapshot.
- **What stays:** `put`, `put --delete`, `land` outside a view, `read` and `log` keep their behaviour. `put` writes to the same draft, for callers without a view.

## Consequences

- An agent changes a repository with ordinary file tools and three verbs: `open`, `land`, and `status` or `diff` when it needs them. No per-file `put` is needed.
- Work survives the agent:
  - Everything snapshotted is on the server.
  - A new session reads the description and the diff, or reopens the view elsewhere.
  - Deleting a view loses only edits made since its last `ch` command.
- A view cannot go stale against its base, because the base is an immutable changeset. It is the copy of server data that ADR 0025 rejected for `ch`, made safe by being pinned to one changeset and discarded with the task.
- `ch` keeps a fourth item of local state, the view's, inside the view rather than in `$XDG_CONFIG_HOME/ch/`. This supersedes ADR 0025's three items and its rejection of a local copy, for views only.
- Its verb list grows by five. This supersedes ADR 0025's statement that the M2 list is the CLI's surface, by adding to it.
- Materialising a view downloads every file of the base. A repository whose size makes that slow is the trigger for the lazy view.
- Two views opened on one workspace overwrite each other's snapshots. One workspace per task is what prevents it; nothing enforces it.
- Snapshot cost grows with the view's file count, since each command scans the view. A cheap first check on size and modification time is the client's to choose.

## Signs the decision was wrong

- Agents snapshot build outputs or temporary files often enough that ignore rules do not keep up, and the landed changesets carry them.
- Agents lose edits because work happens between `ch` commands and a view is discarded first, often enough that a daemon would have paid for itself.
- Agents keep one workspace across tasks and need `update` to follow `main`, so workspaces are being used as branches.
- Snapshot scans or full materialisation dominate an agent's time on real repositories before a lazy view exists.
- Agents ask for partial landing of a workspace instead of opening a second one.
