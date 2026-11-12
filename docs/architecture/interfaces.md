# Interfaces

The ways clients reach the system, which are the `ch` CLI and the HTTP API behind it, and what each may and may not do.

## Current decisions

- **Two first-class interfaces** (ADR 0010, ADR 0026): a minimal CLI named `ch`, and the HTTP API described by OpenAPI. codehub builds no MCP server; anyone may build one outside codehub over the API.
- **Both are driving adapters** under `structure.md`. Each translates between its protocol and the application's driving ports, and holds no subject matter of its own. An operation is added to a port first; an interface then exposes it.
- **Complex work is server-side.** An interface hands a request to the server and reports the outcome. It does not orchestrate multi-step work, and it does not retry or compensate around the server's own steps. The server-side landing automation is described in `source-control.md`.
- **The client is kept to a few verbs.** GitHub's workflow and CLI grew too complex and this project does not carry that over. The verbs in M2 are `login <server url> --issuer <url>`, `use <org>/<repo>`, `put <path> <file>` (with `--executable` for an executable file), `put --delete <path>`, `land`, `log <path>`, `read <path>`, `org create`, `repo create` and `member add|remove`; a draft is named with `--draft <name>`, default `default`. Further verbs are open (ADR 0025).
- **`ch` exits with a code that tells the failure apart:** 0 success, 1 any other failure, 2 usage, 3 not logged in or 401, 4 landing rejected (409 or 403 on `land`), 5 denied or not found on any other operation (403 or 404). `ch --help` lists them.
- **The CLI is named `ch`.** `hub` was rejected because Debian ships a package of that name, GitHub's old CLI.
- **The server API is HTTP with JSON, described by OpenAPI** (ADR 0015). `ch` and any other client call it; its bodies carry metadata, not object bytes.
- **Runners call the same HTTP JSON API** under `/v1/runner/`, as principals (ADR 0034, `runner.md`).
- **Object bytes move by presigned URL** (ADR 0015). The API issues a short-lived presigned URL and the client uploads or downloads the bytes directly to or from the S3-compatible object store, as runners already fetch images (`execution.md`, ADR 0008).
- **The CLI keeps three items of local state**: the server URL, the token and the selected `<org>/<repo>`, in `$XDG_CONFIG_HOME/ch/` with mode 0600. Its output is human text by default and JSON with `--json`. It holds no PostgreSQL or S3 driver and moves object bytes with plain HTTP to presigned URLs (ADR 0025).
- **The API is versioned in the path and described by OpenAPI 3.1** (ADR 0024). Its prefix is `/v1`, resources are under `/v1/orgs/{org}/repos/{repo}/…`, errors are RFC 9457 problem details, and callers present bearer tokens.
- **Access failures are told apart by status** (ADR 0024): 401 without a valid token; 404 when the caller may not read the repository, so its existence is not disclosed; 403 when it may read but not perform the action; 409 for a landing conflict or a lost compare-and-swap, naming the paths.
- **In external authorisation mode every member endpoint answers 403** (ADR 0030, ADR 0024): no caller may administer members through codehub, and the problem detail says members are administered outside codehub, so a client tells it from a denied grant. A request without a valid token is still 401.
- **The OpenAPI document is committed in `crates/adapter-http`** and served at `/v1/openapi.json`; a test asserts they are equal. Landing is a synchronous request in M2.
- **No operation of the API takes a request body on DELETE** (ADR 0032). What identifies the target goes in the path or the query, with the field names the body would have had, and a missing or malformed query parameter is a 400 problem detail.
- **The server binary is `codehub`**, bound to `127.0.0.1` unless configured otherwise (ADR 0024).
- **`ch` takes its token from a file path or an environment variable naming it, and may exchange it by RFC 8693** at a configured token endpoint for a configured audience; the exchanged token is held in memory only. With a source `ch` reads the token on every invocation, does no login, and neither reads nor writes a token in its state file; `--server` (`CH_SERVER`) overrides the stored server. Device-grant login stays for standalone mode (ADR 0031).
- **`ch` authenticates at the token endpoint of its exchange as a client, by HTTP Basic, where given a client identifier and secret.** Both are optional and go together. Keycloak, the first issuer the exchange ran against, refuses an exchange from an unauthenticated caller, so a subject token alone is not enough there; an issuer that wants none is served by giving none.
- **Credentials are not decided here.** What the server accepts is in `access.md`; interfaces carry what it defines.

## Rationale

- Why two first-class interfaces over server-side work: ADR 0010; why neither is an MCP server: ADR 0026.
- The token sources and exchange of `ch`: ADR 0031.
- Why interfaces are adapters over shared ports: ADR 0001 and `structure.md`.
- Why HTTP with JSON and presigned URLs for object bytes, and what would show it wrong: ADR 0015. It was decided before authentication on the API and the runner-to-server transport were settled, although this document's earlier open question tied the choice to them; ADR 0015 names a second transport added on the same endpoint for either of them as a sign the decision was wrong.
- The M2 verbs, local state and output format, and the alternatives not taken: ADR 0025. The API's version, description and status codes: ADR 0024. Why DELETE carries no body: ADR 0032.

## Open questions

Each is settled in an ADR when the project takes a side (`docs/architecture/README.md` rule 4).

- **Further verbs of `ch`** beyond those in M2 (ADR 0025), such as `find` and `run`, and whether `edit` is a verb. Options: a fixed small set; a small set plus an alias or plugin mechanism. Recommendation: a fixed small set.
- **Streaming and long-running operations**, such as reporting a landing and its post-land checks. Options: long-poll; server-sent events; a status resource the client polls. Recommendation: none yet; ADR 0010 counts repeated polling around one workflow as a sign of a wrong unit of work, so the choice is made per operation with that in view.
