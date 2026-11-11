//! The OpenID AuthZEN Authorization API 1.0 implementation of the authorisation decision port:
//! each decision is one Access Evaluation request to a policy decision point.
//!
//! The request is `POST <base>/access/v1/evaluation`, JSON, authenticated with
//! `Authorization: Bearer <token>`, and the answer is `{"decision": <bool>, ...}`: `true` is
//! `Allow` and `false` is `Deny`. The application's terms map to the wire as follows (ADR 0030):
//!
//! | Application | AuthZEN |
//! |---|---|
//! | the principal | subject `{"type": "principal", "id": <subject>, "properties": {"issuer": <issuer>}}` |
//! | the action | `{"name": <name>}`, one of `create_organisation`, `create_repository`, `administer_members`, `read`, `write` |
//! | an organisation `o` | resource `{"type": "organisation", "id": o}` |
//! | a repository | resource `{"type": "repository", "id": "<organisation>/<name>", "properties": {"organisation": <organisation>, "repository": <name>}}` |
//! | a path in a repository | the repository resource with `"path": <path>` added to its properties |
//!
//! A `repository` resource without `path` is asked for an upload and means "anywhere in the
//! repository".
//!
//! The numeric repository id is never sent: the decision point knows repositories by name.
//!
//! The bearer token is obtained from a token endpoint by the OAuth 2.0 client credentials grant
//! (RFC 6749 §4.4, HTTP Basic client authentication), held in memory, and obtained again when it
//! is within a few seconds of its `expires_in` or after the decision point answers 401. Decisions
//! are never cached. Every failure is [`app::port::driven::authorisation_decision::AuthorisationError::Backend`].

mod authorisation_decision;

pub use authorisation_decision::AuthzenAuthorisationDecision;
