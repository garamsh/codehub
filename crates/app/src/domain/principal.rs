//! Who is calling: a principal named by its issuer and subject, and the caller the application
//! established it for.

use std::fmt;

/// Why issuer and subject are not a principal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrincipalError {
    /// The issuer is empty or holds whitespace.
    Issuer,
    /// The subject is empty.
    Subject,
}

impl fmt::Display for PrincipalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Issuer => f.write_str("an issuer is not empty and holds no whitespace"),
            Self::Subject => f.write_str("a subject is not empty"),
        }
    }
}

impl std::error::Error for PrincipalError {}

/// An identity: the subject an issuer vouches for (ADR 0021).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Principal {
    issuer: String,
    subject: String,
}

impl Principal {
    /// The principal `subject` of `issuer`.
    ///
    /// # Errors
    ///
    /// [`PrincipalError::Issuer`] where `issuer` is empty or holds whitespace, and
    /// [`PrincipalError::Subject`] where `subject` is empty.
    pub fn new(issuer: &str, subject: &str) -> Result<Self, PrincipalError> {
        if issuer.is_empty() || issuer.chars().any(char::is_whitespace) {
            return Err(PrincipalError::Issuer);
        }
        if subject.is_empty() {
            return Err(PrincipalError::Subject);
        }
        Ok(Self {
            issuer: issuer.to_owned(),
            subject: subject.to_owned(),
        })
    }

    /// Who vouches for the subject.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// The subject within its issuer.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }
}

/// The issuer, a space, then the subject: stable, and one text per principal because an issuer
/// holds no whitespace.
impl fmt::Display for Principal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.issuer, self.subject)
    }
}

/// A principal the application has established from a presented credential. Only the application
/// constructs one, so a driving operation that takes it acts for a verified identity.
#[derive(Debug)]
pub struct Caller {
    principal: Principal,
}

impl Caller {
    pub(crate) const fn new(principal: Principal) -> Self {
        Self { principal }
    }

    /// Who is calling.
    #[must_use]
    pub const fn principal(&self) -> &Principal {
        &self.principal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_an_empty_or_spaced_issuer_and_an_empty_subject() {
        assert_eq!(Principal::new("", "ada"), Err(PrincipalError::Issuer));
        assert_eq!(Principal::new("a b", "ada"), Err(PrincipalError::Issuer));
        assert_eq!(
            Principal::new("https://id", ""),
            Err(PrincipalError::Subject)
        );
    }

    #[test]
    fn renders_distinct_principals_as_distinct_text() {
        let one = Principal::new("https://id", "a b").unwrap();
        let two = Principal::new("https://id", "a").unwrap();
        assert_eq!(one.to_string(), "https://id a b");
        assert_ne!(one.to_string(), two.to_string());
    }
}
