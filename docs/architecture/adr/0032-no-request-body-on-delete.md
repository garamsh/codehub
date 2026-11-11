# ADR 0032: No HTTP operation of codehub's API takes a request body on DELETE

> Status: accepted
> Date: 2026-11-12

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

ADR 0024 fixed the shape of the HTTP API but not what a request may carry. Two operations took a JSON body on DELETE: removing a member (a principal) and removing a grant. RFC 9110 §9.3.5 (read at rfc-editor.org on 2026-11-12) says that content received in a DELETE request "has no generally defined semantics, cannot alter the meaning or target of the request, and might lead some implementations to reject the request and close the connection because of its potential as a request smuggling attack". An intermediary may therefore drop the body or refuse the request, and the server would act on a request that differs from the one the client sent, or not receive it.

Alternatives not taken:

- Keeping the bodies. It works only where every hop forwards them.
- Moving an issuer into a path segment. An issuer is a URL, and a URL in a path segment needs escaping that servers and proxies handle unevenly.
- Replacing DELETE with POST for the two removals. It gives up the method that says what the request does.

## Decision

- No HTTP operation of codehub's API takes a request body on DELETE. What identifies the target goes in the path or the query.
- A body that a DELETE carried moves to the query string, under the field names of the body's JSON properties. A principal is `issuer` and `subject`.
- PUT and POST keep their bodies.
- A missing or malformed query parameter is a 400 problem detail (ADR 0024), as a malformed body is.
- The committed OpenAPI document declares such parameters with `in: query` and `required: true`, and no `requestBody` on any `delete` operation. A test asserts the second.

## Consequences

- A removal reaches the server through intermediaries that drop or reject DELETE content.
- An issuer and a subject appear in the query string, and so in server and proxy logs, where a body would not.
- A removal that needs more than a query string holds has no body to put it in, and takes a different resource or method.
- `ch member remove` sends the query form, and the API's earlier body form is refused.

## Signs the decision was wrong

- A removal needs a field too large or too structured for a query string.
- Logs of query strings expose values that must not be logged.
- Clients of the API ask for a body on DELETE because their tooling cannot build a query.
