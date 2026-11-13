//! The OpenID AuthZEN Authorization API 1.0 implementation of the authorisation decision port:
//! each decision is one Access Evaluation request to a policy decision point, and decisions on
//! many resources are one Access Evaluations request where the adapter is enabled for it and the
//! decision point offers it (ADR 0038, ADR 0041).
//!
//! The request is `POST <base>/access/v1/evaluation`, JSON, authenticated with
//! `Authorization: Bearer <token>`, and the answer is `{"decision": <bool>, ...}`: `true` is
//! `Allow` and `false` is `Deny`. The names on the wire are those of the [`Profile`]. The `plain`
//! profile, the default, maps the application's terms as follows (ADR 0030):
//!
//! | Application | AuthZEN |
//! |---|---|
//! | the principal | subject `{"type": "principal", "id": <subject>, "properties": {"issuer": <issuer>}}` |
//! | the action | `{"name": <name>}`, one of `create_organisation`, `create_repository`, `administer_members`, `administer_runners`, `read`, `write` |
//! | an organisation `o` | resource `{"type": "organisation", "id": o}` |
//! | a repository | resource `{"type": "repository", "id": "<organisation>/<name>", "properties": {"organisation": <organisation>, "repository": <name>}}` |
//! | a path in a repository | the repository resource with `"path": <path>` added to its properties |
//!
//! A `repository` resource without `path` is asked for an upload and means "anywhere in the
//! repository". The numeric repository id is not sent in this profile: the decision point knows
//! repositories by name.
//!
//! The `grn` profile speaks garam's contract (ADR 0043):
//!
//! | Application | AuthZEN |
//! |---|---|
//! | the principal | subject `{"type": "grn", "id": <subject>}`, no properties |
//! | the action | `codehub:org.use` (`CreateOrganisation`), `codehub:repo.create`, `codehub:repo.read`, `codehub:repo.write`, `codehub:runner.administer` |
//! | an organisation `o` | resource `{"type": "grn", "id": "grn:root/iam/org/<o>"}` |
//! | a repository or a path in one | resource `{"type": "grn", "id": "grn:<organisation>/codehub/repo/<id>"}`, by the repository's id in decimal, no properties |
//! | a path, with `read` and `write` only | `"context": {"path": <path>}`, in an evaluations request on the entry of its resource |
//!
//! `AdministerMembers` has no action in this profile and fails as `Backend`, since external mode
//! never asks it. In both profiles a path that is not valid UTF-8 is denied without asking.
//!
//! Only where `with_access_evaluations(true)` is set, on the first decision on many resources the
//! adapter reads `GET <base>/.well-known/authzen-configuration`.
//! Where it names `access_evaluations_endpoint`, the decisions are one `POST` there, JSON, with
//! the shared `subject` and `action` at the top level and `"evaluations": [{"resource": ...}]`,
//! answered with `{"evaluations": [{"decision": <bool>}, ...]}` in the same order; where it does
//! not, or where it is not enabled, the decisions are one Access Evaluation each. A failed read of
//! the metadata is read again on the next call, and an answer with a different number of
//! decisions than resources is a failure.
//!
//! The bearer token comes from the [`oauth_client::TokenClient`] the adapter is given (ADR 0030,
//! ADR 0042). Where the client holds one token per organisation, a call whose resources span
//! organisations is split by organisation, each group is decided with its organisation's token,
//! the groups are sent one after another, and the answers keep the order of the call. A token the
//! decision point rejects with 401 is obtained again once. Where the token endpoint refuses the
//! organisation's client as `unauthorized_client`, every decision for that organisation is a
//! denial; every other failure is
//! [`app::port::driven::authorisation_decision::AuthorisationError::Backend`]. Decisions are
//! never cached.

mod authorisation_decision;

pub use authorisation_decision::{AuthzenAuthorisationDecision, Profile};
