//! The OpenID Connect implementation of the identity verification port: the application is an
//! OAuth resource server that validates bearer JWTs against an issuer's published keys.

mod identity_verification;

pub use identity_verification::OidcIdentityVerification;
