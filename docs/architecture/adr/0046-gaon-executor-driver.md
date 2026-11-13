# ADR 0046: A gaon executor driver runs each job in a run-once MicroVM, and the job reports its result back to the runner

> Status: accepted
> Date: 2026-11-13

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0008 names three executor drivers: microvm (the default), kubernetes and host. ADR 0034 built only the host driver, which isolates nothing. Goal #91 bounded the work to no microvm executor. garamsh/aurora#749 lifts that bound, and its codehub section asks for "a driver that runs each job in a MicroVM on gaon" (issue #136).

gaon is the orchestrator aurora's cluster runs. What it serves was read at gaon `main` = `2387cebd0b44b322b7da2ff485144a7456a97c6f`, and its PM answered codehub's questions on garamsh/gaon#116 (comment 6844042534):
- A `MicroVM` object (`compute.gaon.garam.sh/v1alpha1`, path `/microvms/{name}`) takes an image, a command, arguments, environment and files at create (gaon ADR 0020). Files total at most 64 files and 1 MiB, and a file may take its content from a `Secret` by reference (`resource-api.md` §MicroVM). A Secret's keys and values together are at most 512 KiB (`resource-api.md` §Secret).
- `runStrategy: Once` runs the entrypoint once. The MicroVM then goes to `Succeeded` on a clean exit or to `Failed` otherwise, and is not started again (`resource-api.md`, phase table; `node-agent.md`, the exit table). It stays until deleted (gaon#116, the gaon PM's triage comment of 2026-11-12, item 5).
- The exit code is not written to `status`: `lastExit` waits on garamsh/mvmd#331 and gaon#73.
- A finished MicroVM's output is neither decided nor served. That is gaon#116 item 2, which waits on garamsh/mvmd#438.
- Without a client certificate, the API server authenticates only a bearer service-account token of its own issuer. Such a token is bound to one MicroVM's name and uid, and reaches that MicroVM's guest as a file at `/var/run/secrets/gaon.garam.sh/serviceaccount/token` that the Node Agent renews (gaon ADR 0029). RBAC authorises its holder through cluster-scoped roles only, with label selectors (`apiserver.md`, `resource-api.md` §Bearer-token access). A rule may name `secrets` in the group `compute.gaon.garam.sh`: the Secret routes are guarded as that resource (`apiserver/httpserver.go` at gaon `79c6b9b`, lines 81–84), and a rule's resources are not restricted (`lib/api/cluster_role.go`, lines 90–99). `resource-api.md` still names only `microvms`, and no gaon test yet applies a selector rule to Secrets; garamsh/gaon#244 adds that test and corrects the sentence.
- Under a label-selector rule (`resource-api.md`, the label-selector rule):
  - a create is judged on the labels in the request body;
  - a get or delete is judged on the stored object's labels, and an object that does not exist counts as unlabelled, so deleting one already gone answers `Forbidden`;
  - a list must carry a label selector within the rule's.
- A file's Secret reference is resolved when the Node Agent creates the MicroVM, and read again while it runs. A missing Secret fails the MicroVM as `Failed` with the reason `ReferenceNotFound` (`node-agent.md` §References; gaon's PM in a message to codehub's PM, 2026-11-13).
- Under gaon's `default` network plugin, egress is one rule for each `--egress-prefix` in the Node's configuration, and ingress is one rule for each port in a MicroVM's `spec.ports` and each `--ingress-prefix`. With no prefix, outside traffic is blocked (`node-agent.md` §Network plugins). Whether aurora's Nodes set them is aurora's deployment fact (gaon#116, comment 6844042534, item 3).

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
  - a token file, read again before each request, as the runner reads its codehub token (ADR 0034). gaon accepts no other credential without a client certificate, so a runner on the gaon driver runs in a gaon MicroVM with a ServiceAccount, and this setting names the token file gaon ADR 0029 places;
  - the job image, by digest;
  - the address to listen on for results, and the URL a job reaches that listener at;
  - a runner name, used to label what it creates. It must be a valid label value, and unique among the runners on one gaon cluster. It is unrelated to the runner's codehub registration.

  A missing setting fails at start. The VMM is gaon's and mvmd's choice, so ADR 0008's open question on the VMM does not bind this driver.

**One job**
1. The driver makes a fresh result secret of 32 random bytes. It creates a `Secret` named `codehub-job-<job id>` that holds the job description under one key: the command, the `CODEHUB_*` values, the files with their addresses and download URLs, the result URL and the result secret. If the serialized description and its key exceed 512 KiB, the driver fails the job as an executor error and creates nothing.
2. It creates a `MicroVM` of the same name with:
   - `runStrategy: Once` and the configured image;
   - the command `codehub-runner run-job --job <path>`;
   - one file at that path, mode 0400, taken from the Secret by reference.

   Both objects carry the label `runner.codehub.garam.sh/name=<runner name>`, so a label-selector RBAC rule can confine the runner to its own objects. The Secret is created first, because the MicroVM's file is resolved from it at create.
3. Inside the MicroVM, `codehub-runner run-job` runs the job as the host driver does: in a fresh directory, files fetched and checked against their addresses, the command under `/bin/sh -c`, and the last 64 KiB of combined output kept. It then posts the end (exit code or signal) and the output tail to the result URL, with the result secret as a bearer token. It retries a post that fails or is not answered with success for up to 30 seconds. It exits with the command's code, or with 1 if no post succeeded.
4. The runner accepts one result per job. It compares the secret in constant time and refuses any other caller. It reads the MicroVM's phase from the API server at a fixed interval until the phase is `Succeeded` or `Failed`.
5. The job ends when the phase is final. A post succeeds before `run-job` exits, so a result sent is held by then. The received result is the job's execution, whichever final phase gaon reports. If the phase is final and no result was received, the driver gives an executor error that names the phase and its reason, such as `ReferenceNotFound`. It never invents an exit code.
6. On cancel or timeout the driver deletes the MicroVM and ends the job as stopped for that reason, as the host driver does.
7. The driver deletes the MicroVM and the Secret after every job, whatever the outcome. The Secret goes only after the MicroVM is gone, since a running MicroVM's file is read from it. Because a delete or get of a missing object answers `Forbidden`, the driver sees an object as gone when a list by its label no longer holds it. A delete that fails is left to the next start's cleanup.

**At start**
- The gaon driver lists the MicroVMs and Secrets carrying its runner name's label and deletes each, MicroVMs first. A runner runs one job at a time (`runner.md`), so anything labelled with its name is left from a run that ended without cleaning up.

**Identity and network**
- The job holds no gaon credential and no codehub credential. The result secret authorises one post for one job and nothing else.
- The runner's listener is the only inbound connection the runner takes, and only a holder of a job's result secret can post to it. The server still never connects to a runner (ADR 0034).
- The listener speaks plain HTTP. TLS would need a certificate for the runner, which neither codehub nor gaon issues it. The secret authorises one post for one job, and the output it carries is what `ch checks` shows the repository's readers.
- The job's command runs as the user that reads the job file, so it can read the result secret. It gains nothing by that: a command can already make its job report success by exiting 0.
- The job's MicroVM must reach the object store's download URLs and the result URL, and the runner's MicroVM must declare the listener's port in its `spec.ports`. Egress prefixes and ingress are the cluster's deployment.

**What this supersedes**
- ADR 0034's "the only driver built is the host driver": the gaon driver is built beside it.
- For the runner's option, ADR 0008's "microvm (default)": the default stays `host`, because the gaon driver cannot run without the gaon settings. The rest of ADR 0008 and ADR 0034 stands.
- The gaon driver is ADR 0008's microvm driver, with gaon placing the MicroVM. It attaches no image, as the host driver attaches none.

## Consequences

- A job on the gaon driver runs isolated from the runner's host and from other jobs, in a MicroVM that is deleted after it ends.
- The exit code and output reach `ch checks` without waiting on mvmd#331, mvmd#438 or gaon#73. If gaon serves them later, the driver can read them as a cross-check, or in place of the report, under a new decision.
- The job image must contain `codehub-runner` and whatever the jobs' commands use. The codehub release builds a runner image that holds `codehub-runner`.
- A job's description must fit a Secret: 512 KiB, of which the files' download URLs take most. A larger changeset fails as an executor error before anything is created. Delivering the source as an image (ADR 0008) is what lifts the bound.
- A command that kills its MicroVM's runner process before it posts reports nothing. The job then ends as an executor error naming the phase.
- Running a job on aurora's cluster also needs:
  - Nodes whose egress prefixes reach the object store and the runner;
  - the runner in a MicroVM with a ServiceAccount and its listener's port;
  - a ClusterRole and ClusterRoleBinding letting that ServiceAccount create, get, list and delete MicroVMs and Secrets carrying its label (resources `microvms` and `secrets`, group `compute.gaon.garam.sh`). The selector rule on Secrets is shown once garamsh/gaon#244 merges.

  All three are aurora's deployment.
- Anyone on the cluster network between a job and its runner can read the result secret and forge a result. TLS on the listener would close that, and needs a certificate the runner does not have.
- Two runners configured with the same name delete each other's live jobs at start.

## Signs the decision was wrong

- gaon serves a finished MicroVM's exit code and output, and the report path then duplicates it with nothing to show for it.
- Job descriptions routinely exceed a Secret's bound before images are delivered.
- Cluster egress policies routinely forbid a MicroVM from reaching its runner.
