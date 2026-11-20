# ADR 0059: each AuthZEN endpoint is pinned to its own CA

> Status: accepted
> Date: 2026-11-21

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

Goal #316 runs codehub against garam, whose token endpoint and policy service are served under CAs that no system root store holds. Two clients need that trust: the token client (`private-key-jwt`, ADR 0042) and the HTTP client of the decision point and the directory (ADR 0041, ADR 0044). The two endpoints are signed by different authorities. The names are agreed in garamsh/discussions#6, discussioncomment-19779853 and discussioncomment-19779859. The owner asked (2026-11-21) how pinning survives a CA rotation and trusting several CAs at once.

Choices considered and not taken:

- **A process-wide bundle (`SSL_CERT_FILE`).** The system roots read it at process start and it replaces them for every client in the process: the OIDC issuer, S3 and any other HTTPS call would trust only that bundle, and one bundle cannot give the token endpoint and the policy service different trust. It stays in the e2e only because the OIDC issuer has no setting of its own.
- **Adding the CA to the system roots (`tls_certs_merge`).** Any system-trusted CA would then still be accepted for a garam endpoint, so the connection is not pinned.
- **One setting for both endpoints.** The two are signed by different CAs; one file would make each trust the other's authority.
- **Skipping verification.** Refused.

## Decision

- **`CODEHUB_AUTHZEN_TOKEN_CA_FILE`** pins the token endpoint and **`CODEHUB_AUTHZEN_CA_FILE`** pins the policy service (decisions and directory). Each is optional. Where set, that client trusts only the certificates in the file and no system root; where unset, it trusts the system roots.
- **A file may hold several certificates** and the client trusts every one of them.
- **The server checks the file at start.** A missing or unreadable file, or one with no certificate, refuses to start, naming the setting and the path.
- **Rotation takes a rollout.** The files are read at start, so a replaced CA reaches pods started after the replacement, as for the key (ADR 0058); the chart adds no checksum of it. A rotation without a gap has two rollouts: put the new CA beside the old one in the file and roll out, then, once the endpoint serves the new chain, remove the old one and roll out again.
- **The chart** takes `authzen.caSecret` and `authzen.tokenCaSecret` (`name`, `key`), mounts each read-only at `/etc/codehub/authzen-ca/ca.crt` and `/etc/codehub/authzen-token-ca/ca.crt` in `api`, `runner-api` and `worker`, and sets the matching variable. Where one names none, nothing is mounted. aurora's Secrets are `garam-policy-ca` and `garam-server-root`, key `ca.crt`, namespace `codehub`.
- **The OIDC issuer is not pinned** by this decision.

## Consequences

- A CA that is system-trusted is refused for a pinned endpoint; an endpoint that moves to a public CA needs the pin removed or the file replaced.
- The OIDC issuer (garam, https) still trusts the system roots, so a deployment whose issuer has a private CA needs the process-wide `SSL_CERT_FILE`, which also changes the roots of every other client. The e2e keeps it for that reason.
- Tests run with no network: a generated CA and a local TLS server, with a child process for the system-trusted case.

## Signs the decision was wrong

- The OIDC issuer needs a pin of its own and `SSL_CERT_FILE` is set in deployments only for it.
- Rotations are missed because a rollout is forgotten, and the operators ask for the chart to watch the Secret.
