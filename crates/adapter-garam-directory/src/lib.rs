//! The garam implementation of the directory-change port: each repository change is one
//! `ApplyEntityChanges` request to garam's policy service (ADR 0044, ADR 0043).
//!
//! The request is `POST <base>/garam/policy/v1/entity-changes`, JSON, authenticated with
//! `Authorization: Bearer <token>` where the token is the change's organisation's, held by the
//! token client. A change is the entity `grn:<organisation>/codehub/repo/<id>` with the change's
//! sequence, as garam's ADR-0149 states:
//!
//! | Change | `ApplyEntityChanges` |
//! |---|---|
//! | created | `upsert` with `org` and one parent, both the entity `grn:root/iam/org/<organisation>`, and no members, consents or operations |
//! | deleted | `delete` |
//!
//! The answer is `{"written": n, "dropped": n, "refusals": [{"index": i, "error": e}]}`. A change
//! is accepted where it was written or dropped, which garam does for a sequence it has applied or
//! passed already, and nothing was refused. Every other answer, and a failure to obtain a token
//! or to reach garam, is not accepted. Organisation changes are ignored: garam holds the
//! organisations.

mod directory_changes;

pub use directory_changes::GaramDirectoryChanges;
