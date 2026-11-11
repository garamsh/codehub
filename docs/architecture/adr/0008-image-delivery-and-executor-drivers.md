# ADR 0008: Images are delivered through object storage and the environment is known only to executor drivers

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

Images derived under ADR 0007 must reach runners that may be on other hosts, and jobs run in different kinds of environment. The design session recorded in issue #1 (item 5) settled delivery and where environment knowledge lives.

## Decision

- Images live in object storage behind an S3-compatible API. A runner fetches them through short-lived presigned URLs, caches them on its host keyed by version hash, and refetches only when the version changes. Images stay as image files and are not extracted.
- The server and the runner core do not know the execution environment. Environment knowledge lives only in executor drivers. Each driver both runs the job and attaches the images. The drivers are microvm (default), kubernetes (images attached via CSI) and host.
- The VMM is not chosen.

## Consequences

- Adding an environment means adding a driver; the server and runner core do not change.
- Authorisation for image bytes is a URL the server issues, so the server decides who may fetch.
- A runner host needs disk for its image cache.
- Not extracting images keeps fetch cost proportional to image size, not file count, and ties the image format to what each driver can attach.

## Signs the decision was wrong

- A driver needs information about the environment from the server or runner core to work, so the boundary leaks.
- Presigned-URL fetches become a bottleneck or an availability dependency that a different delivery path would not have.
- Image files cannot be attached by one of the three drivers without extraction.
- Most jobs use a different driver than the default, so the default was not.
