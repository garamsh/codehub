//! The RFC 7662 implementation of the identity verification port: the application asks the
//! issuer's introspection endpoint whether a bearer token is active and for whom it was issued.

mod identity_verification;

pub use identity_verification::IntrospectionIdentityVerification;
