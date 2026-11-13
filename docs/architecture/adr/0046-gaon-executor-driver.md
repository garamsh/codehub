# ADR 0046: A gaon executor driver runs each job in a run-once MicroVM, and the job reports its result back to the runner

> Status: accepted
> Date: 2026-11-13

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0008 names three executor drivers: microvm (the default), kubernetes and host. ADR 0034 built only the host driver, which isolates nothing. Goal #91 bounded the work to no microvm executor. garamsh/aurora#749 lifts that bound, and its codehub section asks for "a driver that runs each job in a MicroVM on gaon" (issue #136).

gaon is the orchestrator aurora's cluster runs. What it serves was read at gaon `main` = `2387cebd0b44b322b7da2ff485144a7456a97c6f`, and its PM answered codehub's questions on garamsh/gaon#116 (comment 6844042534):
- A `MicroVM` object (`compute.gaon.garam.sh/v1alpha1`, path `/microvms/{name}`) takes an image, a command, arguments, environment and files at create (gaon ADR 0020). Files total at most 64 files and 1 MiB, and a file may take its content from a `Secret` by reference.
- `runStrategy: Once` runs the entrypoint once. The MicroVM then goes to `Succeeded` on a clean exit or to `Failed` otherwise, and stays until deleted (`resource-api.md`, phase table).
- The exit code is not written to `status`: `lastExit` waits on garamsh/mvmd#331 and gaon#73.
- A finished MicroVM's output is neither decided nor served. That is gaon#116 item 2, which waits on garamsh/mvmd#438.
- The API server authenticates a bearer service-account token without a client certificate. RBAC authorises it, with label selectors (`apiserver.md`).
- Egress from a MicroVM is a per-MicroVM network policy of the cluster's deployment, not of gaon's API (gaon#116 item 4).

So gaon can run a job's command in a MicroVM and say whether it exited cleanly. It cannot yet return the exit code or the output, which `ch checks` shows (ADR 0034).

Alternatives not taken:

- **Wait for gaon to serve the exit code and the output.** codehub's criterion would then wait on two issues in mvmd and two in gaon that have no date. The driver would also learn nothing more than a job can tell it directly.
- **Have the job report to the codehub server itself.** It would need a credential for the job's own MicroVM that the server accepts, which is a second kind of runner identity (ADR 0034 made a runner one principal). The lease and heartbeat would also have two owners.
- **Put the job description in the MicroVM's inline files.** The download URLs and the result secret would then be readable by anyone allowed to read MicroVMs. A Secret is readable only under its own rule.
- **A Kubernetes driver.** aurora's cluster runs gaon, not Kubernetes.

## Decision

**Selection**
- `codehub-runner` takes `--executor host|gaon`, default `host`. With `gaon`, it requires:
  - the API server's URL;
  - a CA file to trust;
  - a token file, read again before each request, as the runner reads its codehub token (ADR 0034);
  - the job image, by digest;
  - the address to listen on for results, and the URL a job reaches that listener at;
  - a runner name, used to label what it creates.

  A missing setting fails at start. The VMM is gaon's and mvmd's choice, so ADR 0008's open question on the VMM does not bind this driver.

**One job**
1. The driver makes a fresh result secret of 32 random bytes. It creates a `Secret` named `codehub-job-<job id>` that holds the job description: the command, the `CODEHUB_*` values, the files with their addresses and download URLs, the result URL and the result secret.
2. It creates a `MicroVM` of the same name with:
   - `runStrategy: Once` and the configured image;
   - the command `codehub-runner run-job --job <path>`;
   - one file at that path, taken from the Secret by reference.

   Both objects carry the label `runner.codehub.garam.sh/name=<runner name>`, so a label-selector RBAC rule can confine the runner to its own objects.
3. Inside the MicroVM, `codehub-runner run-job` runs the job as the host driver does: in a fresh directory, files fetched and checked against their addresses, the command under `/bin/sh -c`, and the last 64 KiB of combined output kept. It then posts the end (exit code or signal) and the output tail to the result URL, with the result secret as a bearer token. It exits with the command's code.
4. The runner accepts one result per job. It compares the secret in constant time and refuses any other caller. It reads the MicroVM's phase from the API server at a fixed interval until the phase is `Succeeded` or `Failed`.
5. A received result is the job's execution. If a final phase arrives with no result, the driver gives an executor error that names the phase. It never invents an exit code.
6. On cancel or timeout the driver deletes the MicroVM and ends the job as stopped for that reason, as the host driver does.
7. The driver deletes the MicroVM and the Secret after every job, whatever the outcome.

**At start**
- The gaon driver deletes every MicroVM and Secret carrying its runner name's label. A runner runs one job at a time (`runner.md`), so anything labelled with its name is left from a run that ended without cleaning up.

**Identity and network**
- The job holds no gaon credential and no codehub credential. The result secret authorises one post for one job and nothing else.
- The runner's listener is the only inbound connection the runner takes, and only jobs it created can call it. The server still never connects to a runner (ADR 0034).
- The MicroVM must reach the object store's download URLs and the result URL. Allowing that is the cluster's egress policy, a deployment fact.

**What this supersedes**
- ADR 0034's "the only driver built is the host driver": the gaon driver is built beside it.
- For the runner's option, ADR 0008's "microvm (default)": the default stays `host`, because the gaon driver cannot run without the gaon settings. The rest of ADR 0008 and ADR 0034 stands.
- The gaon driver is ADR 0008's microvm driver, with gaon placing the MicroVM. It attaches no image, as the host driver attaches none.

## Consequences

- A job on the gaon driver runs isolated from the runner's host and from other jobs, in a MicroVM that is deleted after it ends.
- The exit code and output reach `ch checks` without waiting on mvmd#331, mvmd#438 or gaon#73. If gaon serves them later, the driver can read them as a cross-check, or in place of the report, under a new decision.
- The job image must contain `codehub-runner` and whatever the jobs' commands use. The codehub release builds a runner image that holds `codehub-runner`.
- A job's description must fit gaon's file bound: 1 MiB, of which the files' download URLs take most. A larger changeset fails as an executor error before the MicroVM is created. Delivering the source as an image (ADR 0008) is what lifts the bound.
- A command that kills its MicroVM's runner process before it posts reports nothing. The job then ends as an executor error naming the phase.
- Running a job on aurora's cluster also needs the cluster's egress policy and an RBAC rule for the runner's ServiceAccount. Both are aurora's deployment.

## Signs the decision was wrong

- gaon serves a finished MicroVM's exit code and output, and the report path then duplicates it with nothing to show for it.
- Job descriptions routinely exceed gaon's file bound before images are delivered.
- Cluster egress policies routinely forbid a MicroVM from reaching its runner.
