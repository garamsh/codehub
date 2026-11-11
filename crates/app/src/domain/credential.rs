//! The credential a caller presents with a request.

use std::fmt;
use std::str::FromStr;

/// Why text is not a credential: it is empty.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EmptyCredential;

impl fmt::Display for EmptyCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a credential is not empty")
    }
}

impl std::error::Error for EmptyCredential {}

/// An opaque bearer string, as presented. Its [`Debug`] form hides it.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Credential(String);

impl Credential {
    /// The credential as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Credential {
    type Err = EmptyCredential;

    fn from_str(text: &str) -> Result<Self, EmptyCredential> {
        if text.is_empty() {
            Err(EmptyCredential)
        } else {
            Ok(Self(text.to_owned()))
        }
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credential(..)")
    }
}
