# ADR 0061: the release tag versions the chart, which is published to ECR and pinned by digest

> Status: accepted
> Date: 2026-11-24

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The cluster that runs codehub (aurora, goal #316) takes charts as OCI artifacts from ECR and pins each by digest. The chart in `deploy/helm/codehub` carries a fixed `version` and `appVersion` in `Chart.yaml`, so a release's images and the chart that deploys them had no shared version. Another project depends on where the chart is and how it is named, which makes this an interface.

Choices considered and not taken:

- **Bump `Chart.yaml` by hand with each release.** The tag and the file can disagree, and the registry is tag-immutable, so a wrong version cannot be pushed again.
- **Push from the `images` job.** The job's outputs and permissions are the images'; a chart failure would sit among them.

## Decision

- **The release tag sets the chart's `version` and `appVersion`** to the tag without its `v`, over what `Chart.yaml` holds, at `helm package --version --app-version`. `Chart.yaml`'s own values are not released.
- **The chart is pushed to `codehub/charts/codehub:<version>`** in the registry of the images, by the `chart` job of `.github/workflows/release.yml` with the images' push role.
- **The repository is tag-immutable**, so a version is pushed once; a failed push is repaired by re-running the failed job only if nothing was pushed, otherwise by the next version.
- **Aurora pins the chart by digest.** The job reads the digest back from the registry and writes it to the job summary and the GitHub release body.

## Consequences

- The GitHub release is created after the chart job, but a release can exist while its chart is not yet pushed if the chart job is re-run later; a consumer reads the digest from the release body and treats a release without one as incomplete.
- A chart change reaches the cluster only with a tag.

## Signs the decision was wrong

- A chart fix has to ship without an image change, and a version bump per fix is too heavy.
