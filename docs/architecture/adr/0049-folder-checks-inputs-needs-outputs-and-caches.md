# ADR 0049: Checks are declared per folder with file inputs, run in `needs` order, hand on their outputs, start from cache refs and are skipped when their result is cached

> Status: accepted
> Date: 2026-11-14

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0035 declares a repository's checks once, in `.codehub/checks.toml` at the root, and queues every check on every landing. A runner is granted every file of the changeset's tree. Nothing orders one check after another, nothing passes a build product from one check to the next, and a check whose inputs did not change runs again. In a monorepo, every landing costs the whole repository.

The owner decided the model on 2026-11-14 (issue #191, its decisions 1–9; goal issue #192). The PM was granted the details within it; those taken are marked "detail" below.

Systems compared:
- **Google's TAP over Blaze/Bazel and Forge.** It schedules by affected build targets and caches action results by content. It is internal to Google, and it requires a hermetic build declared down to the file.
- **GitHub Actions.** `paths:` filters select workflows and `needs` orders jobs. It has no result cache keyed by content, and a job clones the repository.
- **Nx and Turborepo.** These compute the affected projects and cache task results by their inputs. They run inside one build tool's workspace.

Alternatives not taken:
- **Bazel-style build targets declared per file.** The users of a repository would have to adopt one build system before checks could be selective.
- **An external CI service in TAP's place.** Selection and caching depend on the changeset model, which only the server holds.
- **Dependencies inferred from the code.** A wrong inference cannot be found by reading a file, and hermetic grants need a declared list.
- **A `with`/`triggers` relation for checks that run together.** It expresses nothing that `inputs` cannot express, and it would give users two ways to say one thing.
- **Default inputs per language** (for example `//Cargo.lock` added automatically). It is an exception to "a check sees what it declares". A missing file fails in the runner where it is visible, and `ch checks plan` lists each check's inputs.
- **Copy-on-write volumes for build caches.** naru has no API to clone a disk or to attach an existing one to a new microVM, and its volumes do not move between hosts (aurora's PM, issue #191 issuecomment-6872416106). A cache kept as a tree in the content-addressed store needs neither.

## Decision

**Declaration**
- Any folder may hold `.codehub/checks.toml`. The file at the root keeps ADR 0035's meaning, and its checks keep their bare names. A check in another folder is named `<folder>:<name>`, for example `services/api:test` (detail).
- A check has ADR 0035's fields: a name, a command, labels, `blocking` and a timeout. It may also have `inputs`, `needs`, `outputs` and `caches`.
- **`inputs`** are path patterns relative to the folder, or relative to the repository root when they start with `//`. Without `inputs`, a check's input is its whole folder.
- **`needs`** names checks that must pass before this one starts. A name with no `//` is a check in the same folder; `//lib:c` names check `c` of the folder `lib`, and `//:unit` names a root check (detail).
- **`outputs`** are paths, relative to the folder, that the check produces.
- **`caches`** are paths, relative to the folder, that the check uses as a build cache.
- These definitions are unreadable when:
  - a `needs` names no check;
  - `needs` form a cycle;
  - the file does not parse (as before).

  Unreadable definitions count as one failed blocking check, named for the file that declares them, as ADR 0035 provides for a file that does not parse.

**Keys and the result cache**
- Each check of a landed changeset has a **key**, made from:
  - its definition;
  - the path and content address of each file its `inputs` match;
  - the key and output tree of each check it `needs`.
- **A check whose key already passed is not run.** Its result is recorded as passed from the cache, with the output tree of the run that passed.
- A failed, timed-out or cancelled run is not cached, so a later landing with the same key runs it again (detail).
- The result cache is per repository.
- A check whose inputs and `needs` did not change since a run that passed is therefore not run. This is how "a check runs when its inputs change" is realised, and the first landing of a check always runs it.

**Order**
- A check is queued once every check it `needs` has passed, whether by a run or from the cache. Checks with no `needs` between them run in parallel.
- When a check it `needs` fails, times out or is cancelled, the check is not run. It is recorded as **skipped** and as not passed (detail).
  - `last-green` does not move past a changeset with a skipped blocking check.
  - A skipped check reverts nothing. The revert, if there is one, belongs to the check that failed (ADR 0035).

**What a runner receives**
- A job is granted only these files:
  - the files its `inputs` match, at the landed changeset;
  - the output trees of the checks it `needs`, at the paths those checks declared;
  - the current tree of each of its cache refs.

  It is granted no other file of the repository, and it clones nothing.
- The job's environment carries `CODEHUB_ORGANISATION`, `CODEHUB_REPOSITORY`, `CODEHUB_CHANGESET` and `CODEHUB_CHECK`, as before, and `CODEHUB_FOLDER`, the check's folder (detail). A script needs no git.
- On a clean exit, the runner uploads the files under the job's `outputs` that the store lacks. The server records the output tree with the result.

**Cache refs**
- A cache ref maps (repository, folder, cache path) to a tree in the content-addressed store.
- A job starts with each of its cache refs' trees written into its cache paths, and may change them.
- The ref advances to the tree a job leaves only when both hold:
  - the job passed;
  - its changeset is later on `main` than the changeset the ref records.

  Otherwise the job's cache changes are dropped. The newest landing wins, and trees are never merged. This is ADR 0007's promotion of a cache limited to trusted jobs, with "trusted" meaning a post-land job on `main`.
- A cache is not part of a check's key: it makes a run faster, and must not change its result.
- A job on a changeset not on `main` reads its cache refs and drops its changes. Today every job is post-land (ADR 0035), so this rule waits for such jobs.

**Planning**
- `ch checks plan` asks the server for the plan of the caller's draft. For each check, in `needs` order, the plan says whether the check would run or is a cache hit, and why it would run. Keys are computed against the draft's tree as if it landed now (detail).

## Consequences

- A landing costs the checks whose inputs it changed and the checks that need them, not the whole repository.
- A check's grant is its declaration. A check that needs an undeclared file fails in the runner. A check that only reads such a file when it is present passes without it, and its key does not change when the file does.
- A check that `needs` another receives that check's build products, so an end-to-end check declares its own test files as `inputs` rather than the sources its upstream already built.
- Build caches persist across jobs without volumes, attached disks or host affinity. Each job downloads its cache trees, and a cache costs transfer time on every job until hosts keep a local copy of the store.
- A non-deterministic check that once passed stays passed until one of its inputs changes.
- The key depends on the output trees of the checks a check `needs`. An upstream that produces different bytes from the same inputs, for example by embedding a timestamp, runs its dependants again whenever it runs.

## Signs the decision was wrong

- Users widen `inputs` to `//**` to make checks pass, which brings back the cost of the whole repository.
- Cache hits hide breakage: a check that should have failed stays passed because its declared inputs missed a file it depends on.
- Cache downloads dominate job time often enough that users drop `caches`.
- `needs` graphs grow deep enough that `main`'s checks take longer than without ordering.
