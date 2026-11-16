# ADR 0051: A run executes a command against the caller's workspace on a runner, writes changed files back by compare-and-swap, and the runner caches objects by address

> Status: accepted
> Date: 2026-11-16

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0048 put an agent's work in a workspace held on the server, with a disposable view on the agent's side. Builds and tests still happen wherever the view is, so every agent machine needs the toolchain and downloads the whole base to build. codehub holds the code on the server, after Google's Piper and CitC, and builds follow it there, as Google's do on Forge.

The owner decided the model on 2026-11-16 (issue #244, goal issue #245, its decisions 1–9). The PM was granted the details within it; those taken are marked "detail" below.

What existed before:
- A runner job ran only a post-land check against a landed changeset (ADR 0035, ADR 0049).
- The runner downloaded every granted file into a fresh directory for each job, with no local copy kept between jobs (`runner.md`, the host driver).
- A job kept the last 64 KiB of its combined output.
- A draft had no version, so nothing could tell whether it changed between two moments.

ADR 0048 rejected "no view, with builds run remotely as jobs", because every edit-and-test loop would become a remote round trip. This decision keeps the view: an agent edits locally and runs remotely only when it chooses, so the round trip is paid per run, not per edit, and against a machine without the toolchain or the download it is the cheaper one.

Systems compared:
- **Google's Forge and CitC.** A build reads the workspace on the server and writes its outputs to a content-addressed store; the developer fetches what they need.
- **Bazel's remote execution API.** An action names its input tree by digest, the executor keeps a content-addressed cache, and outputs are fetched on demand.
- **GitHub Codespaces and similar remote environments.** A whole machine per developer, holding a clone. The environment, not the repository, is the unit.

Alternatives not taken:
- **A remote shell or a long-lived environment per workspace.** It brings back a machine an agent must manage and keep in step with its view.
- **Streaming output while the run goes.** Decision 3 leaves it out; `interfaces.md`'s open question on streaming stays open.
- **Writing back every file the command creates.** Build products and temporary files would land in the workspace; decision 5 writes back only tracked files and those named with `--add`.
- **Merging a write-back into a workspace that changed meanwhile.** It would apply a formatter's output over edits the formatter never saw; decision 6 refuses instead.
- **Write including run.** Decision 4 keeps `run` apart, so an organisation can grant edits without granting the right to execute on its runners.
- **A new route of job outside the check path.** Decision 8 keeps one claim, heartbeat and result conversation for every job.
- **Hard links from the object cache into a job's directory.** A command that writes a file in place would change the cached copy, and the cache would serve wrong bytes under a correct address (detail).

## Decision

**A workspace has a version and an instance** (detail)
- A workspace's version is a number that starts at 0 when it opens and grows by one each time its changes are recorded: a snapshot, a `put`, a restore or a write-back. Setting the description does not change it. `GET drafts/{draft}` answers it.
- Each opening of a workspace is an **instance**. A workspace that lands closes its instance, and opening the same name again starts a new one at version 0. A run is bound to the instance it started in.

**A run**
- A run executes one command against one workspace of the caller, on a runner. It is a job on the existing claim, heartbeat and result path, an unnamed check on the workspace (decision 8). Unlike a post-land check, it is not blocking, reverts nothing, enters no result cache, does not move `last-green`, and is not listed by `ch checks`. The job queue holds its record.
- `POST /v1/orgs/{org}/repos/{repo}/drafts/{draft}/runs` starts one (detail). Its body carries:
  - `command`, run under `/bin/sh -c` as a check's command is;
  - an optional `prefix`;
  - `add`, a list of paths, each under `prefix` where one is given;
  - `outputs`, a list of paths.

  Every path is relative to the repository root and is validated as a path value (`source-control.md`); an invalid one is a 400 problem detail. It answers the run, with an `id`.
- **Input** (decision 2). The run's files are the workspace's tree, its base with its changes applied, at the version current when the run starts, limited to `prefix` where given. The server records the run's files, the instance and that version with the run.
- **Settings** come from `.codehub/run.toml` at the root of the workspace's tree, read when the run starts, whatever the prefix (decision 7; detail beyond `caches`):
  - `caches`, paths relative to the root used as build caches, default none;
  - `labels`, the labels a runner must have, default none;
  - `timeout`, in seconds, default 600, as a check's.

  A file that does not parse refuses the run with a 400 problem detail naming the file. Where the file is absent, every field takes its default.
- **Permission.** Starting a run asks, in one decision:
  - `run` on the repository (decision 4);
  - `read` on every path the run is sent, and on `.codehub/run.toml` where it exists;
  - `read` on the whole repository, where a cache ref of the run would be seeded from `main`'s (below).

  A caller refused any of these is refused the run, and no job is queued.
- **Runners.** A run's job is handed only to a runner whose executor is `host` (decision 8). The runner states its executor when it claims; a runner on the gaon driver is never handed one.
- `GET runs/{id}` (under the repository) answers the run, and with `wait=<seconds>`, at most 60, holds the request until the run ends or the time passes, as a runner's claim is held (detail).
- `POST runs/{id}/cancel` cancels the run (detail).
- Every route under `runs/{id}`, `runs/{id}/files` included, answers only the principal that started the run, and 404 to any other caller, so a run's existence is not disclosed.

**What a run's job receives**
- The run's files, each with a download grant.
- The trees of the workspace's cache refs, written into their paths.
- The command, `add`, `outputs` and the timeout.
- `CODEHUB_ORGANISATION`, `CODEHUB_REPOSITORY` and `CODEHUB_DRAFT` in the command's environment, with `CODEHUB_CHANGESET` set to the workspace's base, and `CODEHUB_CHECK` and `CODEHUB_FOLDER` unset (detail).

**What a run reports** (decision 3)
- The command's exit code; for a command ended by a signal, 128 plus the signal's number, as a shell gives it. Otherwise: that it timed out, was cancelled, or failed in the executor.
- Its standard output and standard error, kept apart, each uploaded by the runner as one object and recorded by address. Each is kept whole up to 64 MiB; past that, the runner keeps the last 64 MiB and the run records that the stream was cut (detail). A job of a check keeps its 64 KiB tail as before.
- The object counts: how many of the job's objects the runner downloaded, and how many it took from its object cache.
- The **output tree**: the regular files under the run's `outputs` and its cache paths when the command ended, recorded as an encoded tree (ADR 0016), so every path in it is a valid path value.
- The **changes**: each sent path whose content or kind differs from what it was sent as, or which is gone, and each path under `add` that exists, with its kind and address. Paths under a cache path are never changes.
  - The runner reads each path without following a symbolic link. A sent or `add` path that holds anything other than a regular file, a symbolic link included, is not reported as a change, and the run records it as refused.
  - The runner uploads the bytes the object store lacks before it reports.

**Write-back** (decisions 5 and 6)
- A run whose changes are empty writes nothing back and conflicts with nothing, whatever happened to the workspace meanwhile.
- When the command ended with an exit code, whatever the code, the server applies non-empty changes to the workspace, only if all hold:
  - the run's instance is still open;
  - the workspace's version is still the one the run started from;
  - the caller may `write` every changed path, asked in one decision when the run ends.

  It is applied as one snapshot, all together, and the version grows by one.
- Otherwise nothing is written back, and the run records why: the instance closed; the workspace moved, naming the paths it changed since; or a write was denied, naming the paths.
- A run that timed out, was cancelled or failed in the executor writes nothing back.
- A sent path that is gone is written back as a deletion, or as a restore where the base lacks it. A path the base holds and the run left with the base's content is written back as a restore, as a snapshot sends it (ADR 0048).

**Workspace cache refs** (decision 7)
- A workspace instance has a cache ref for each cache path `run.toml` names, created by the first run that names the path. It maps (repository, workspace instance, cache path) to a tree.
- A new ref starts from the root folder's cache ref of the same path on `main` (ADR 0049), or empty where `main` has none. A caller without `read` on the whole repository is refused a run that would seed a ref from `main`'s, since `main`'s caches are built from files it may not read.
- When a run ends with an exit code, whatever the code, and its instance is still open, each of its cache refs moves to the tree the run left, by compare-and-swap from the tree the run started from. A ref another run moved meanwhile is not moved, and the newer tree stays. Trees are never merged.
- This refines ADR 0049's rule that a job on a changeset not on `main` drops its cache changes, and ADR 0007's limit of promotion to trusted jobs. A workspace's ref is read only by that workspace's own runs, started by the principal whose commands wrote it, so no other principal's job reads what it poisons.
- A run never moves a cache ref of `main`. A workspace's refs are removed when its instance closes.
- A cache is not written back into the workspace.

**`ch`**
- `ch run [--draft <name>] [--prefix <path>] [--add <path>]... [--output <path>]... -- <command>...` starts a run, waits for it, writes its standard output and standard error to its own, then prints `exit <code>` and any reason for no write-back on standard error (decision 3). The command's words are joined with spaces into one shell command (detail).
  - In a view it first snapshots the view, and uses the view's draft and, unless given, its prefix. This adds `run` to ADR 0048's list of the commands that snapshot.
  - After a write-back, it writes each changed file into the view only where the view's file is still what the view last recorded. A file edited in the view during the run is left as it is and named on standard error, so the next snapshot sends the edit over the write-back. The files it writes are recorded as the view's last snapshot.
  - An interrupt cancels the run, then waits for it to end.
  - Its exit code (detail):
    - the command's own code, where the command ended with one and nothing that changed failed to be written back;
    - **122** where the command ended but its changes were not written back because a write was denied;
    - **123** where the command ended but its changes were not written back because the workspace moved or its instance closed (the conflict of decision 6);
    - **124** where the run timed out or was cancelled;
    - **125** where `ch` or the server failed before the command could run, including a refusal, with ch's usual message;
    - **126** where something failed after the command started: the executor failed, the run's output could not be read, or `ch` lost the run.

    ch's own codes 2 to 5 would be read as the command's, so `run` does not use them. A command whose own code is between 122 and 126 is told apart from these by `--json`.
  - `--json` prints the run as the server answers it, in place of the output.
- `ch fetch <path> [<dest>] [--draft <name>] [--run <id>]` downloads the file, or every file under the directory, at `<path>` in the output tree of the workspace's latest ended run, or of the run named, into `<dest>`, default the path's last component in the current directory. An executable file is written executable (detail).
- `GET drafts/{draft}/runs/latest` answers the workspace's latest ended run, and `GET runs/{id}/files` lists its output tree with a download grant for each file, under an optional `prefix` (detail).

**The `run` action** (decision 4)
- `run` is an action on a repository. It is granted to a member for the whole repository only: a `run` grant carries the empty prefix, and one with any other prefix is refused (detail).
- Neither `write` nor `read` includes it. An organisation admin holds it.
- The AuthZEN plain profile names it `run`, as its other actions are named in lowercase, with no path property. The GRN profile names it `codehub:repo.run`, with no `context.path` (detail).

**The runner's object cache** (detail)
- Every job a runner runs, a run's or a check's, takes its files through the cache.
- The cache is a directory of files named by their address under `--work-dir` (`objects/`).
- An object is written there only after its bytes are checked against its address, and is copied into the job's directory, never linked.
- It is bounded by `--object-cache-size`, default 10 GiB. After each job the least recently used objects are removed until the cache is within the bound; using an object counts as using it.
- The runner counts, for each job, the objects it downloaded and those it took from the cache, logs both, and reports them with the result.

## Consequences

- An agent can build, test and format with the toolchain on a runner, and its machine needs only `ch`.
- A formatter's output reaches the workspace without a second step. A change the agent recorded during the run stops the write-back, and one it made in the view without recording is left in the view.
- A second run in the same workspace downloads only the objects the first did not, and its build starts from the first run's caches.
- A member who may read only part of a repository runs with no caches seeded from `main`, or not at all where `run.toml` names caches; a first run of such a member builds from nothing.
- A run costs a runner job: it waits behind landings' checks for the same runners, and the organisation pays for its runners' time.
- Output that is not streamed means an agent sees nothing of a long run until it ends.
- Write-back runs whatever the command's exit code, so a command that fails part-way can leave half-written files in the workspace. `ch status` and `ch diff` show them, and `ch restore` removes them.
- The host driver runs a command as the runner's user, with no isolation (`execution.md`). Granting `run` to a principal is trusting it with what that runner can reach.

## Signs the decision was wrong

- Agents add `--add` for files they did not mean to keep often enough that build products reach landings.
- Write-backs are refused for a moved workspace often enough that agents stop editing while a run goes, or re-run to get the write-back.
- Agents run long commands and ask for output while they run, which is the trigger for streaming.
- Runner object caches evict the objects of a repository's next run before it comes, so the counts show no saving.
- Runs crowd out post-land checks on shared runners, and organisations ask for runners separate by kind of job.
