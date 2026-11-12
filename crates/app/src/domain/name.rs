//! Names the outside chooses: organisations, repositories and drafts. Each is parsed once and
//! trusted after.

use std::fmt;
use std::str::FromStr;

/// The longest name, in bytes.
const MAX_LEN: usize = 63;

/// Why text is not a name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NameError {
    /// The name is empty or longer than 63 bytes; carries its length.
    Length(usize),
    /// The name holds a byte other than a lowercase ASCII letter, a digit or `-`.
    InvalidByte(u8),
    /// The name starts with `-`.
    LeadingHyphen,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length(length) => write!(f, "a name is 1 to {MAX_LEN} bytes, not {length}"),
            Self::InvalidByte(byte) => write!(
                f,
                "a name holds lowercase ASCII letters, digits and '-', not byte {byte:#04x}"
            ),
            Self::LeadingHyphen => f.write_str("a name starts with a letter or a digit"),
        }
    }
}

impl std::error::Error for NameError {}

fn check(text: &str) -> Result<(), NameError> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_LEN {
        return Err(NameError::Length(bytes.len()));
    }
    if let Some(byte) = bytes
        .iter()
        .find(|byte| !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || **byte == b'-'))
    {
        return Err(NameError::InvalidByte(*byte));
    }
    if bytes.first() == Some(&b'-') {
        return Err(NameError::LeadingHyphen);
    }
    Ok(())
}

macro_rules! name {
    ($(#[$meta:meta])* $type:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $type(String);

        impl $type {
            /// The name as text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $type {
            type Err = NameError;

            fn from_str(text: &str) -> Result<Self, NameError> {
                check(text)?;
                Ok(Self(text.to_owned()))
            }
        }

        impl fmt::Display for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

name! {
    /// The name of an organisation: 1 to 63 bytes of lowercase ASCII letters, digits and `-`,
    /// starting with a letter or a digit.
    OrganisationName
}

name! {
    /// The name of a repository inside its organisation; the form of an [`OrganisationName`].
    RepositoryName
}

name! {
    /// The name a principal gives a draft; the form of an [`OrganisationName`].
    DraftName
}

name! {
    /// The name of a check inside its repository's check definitions; the form of an
    /// [`OrganisationName`].
    CheckName
}

impl CheckName {
    /// The name of the check that stands for check definitions that could not be read.
    pub(crate) fn unreadable() -> Self {
        Self("checks-file".to_owned())
    }
}

name! {
    /// A label a check requires of a runner and a runner offers; the form of an
    /// [`OrganisationName`].
    Label
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_letters_digits_and_inner_hyphens() {
        assert_eq!("a1-b2".parse::<RepositoryName>().unwrap().as_str(), "a1-b2");
        assert!("0".parse::<OrganisationName>().is_ok());
        assert!("a".repeat(63).parse::<DraftName>().is_ok());
    }

    #[test]
    fn the_unreadable_check_name_is_a_valid_name() {
        let name = CheckName::unreadable();
        assert_eq!(name.as_str().parse::<CheckName>().unwrap(), name);
    }

    #[test]
    fn refuses_what_is_not_a_name() {
        assert_eq!("".parse::<RepositoryName>(), Err(NameError::Length(0)));
        assert_eq!(
            "a".repeat(64).parse::<RepositoryName>(),
            Err(NameError::Length(64))
        );
        assert_eq!(
            "Acme".parse::<OrganisationName>(),
            Err(NameError::InvalidByte(b'A'))
        );
        assert_eq!(
            "a_b".parse::<OrganisationName>(),
            Err(NameError::InvalidByte(b'_'))
        );
        assert_eq!(
            "é".parse::<OrganisationName>(),
            Err(NameError::InvalidByte(0xc3))
        );
        assert_eq!("-a".parse::<DraftName>(), Err(NameError::LeadingHyphen));
    }
}
