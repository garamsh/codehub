//! Content addresses: the SHA-256 of an object's bytes (ADR 0011, ADR 0016).

use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};

/// The SHA-256 address of a blob or an encoded object, written `sha256:<hex>`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Address([u8; 32]);

impl Address {
    /// The address of `bytes`: a blob's content, or an object's encoding.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// The address whose SHA-256 digest is `digest`.
    #[must_use]
    pub const fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// The address's 32 digest bytes.
    #[must_use]
    pub const fn digest(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Why text is not an address.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AddressError {
    /// The text does not start with `sha256:`.
    MissingPrefix,
    /// The text after the prefix is not 64 digits; carries its length in bytes.
    Length(usize),
    /// The text holds a byte other than `0`-`9` and `a`-`f`; carries it.
    InvalidDigit(u8),
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPrefix => f.write_str("an address starts with \"sha256:\""),
            Self::Length(length) => {
                write!(
                    f,
                    "an address has 64 hex digits after its prefix, not {length}"
                )
            }
            Self::InvalidDigit(byte) => write!(
                f,
                "an address holds lowercase hex digits, not byte {byte:#04x}"
            ),
        }
    }
}

impl std::error::Error for AddressError {}

impl FromStr for Address {
    type Err = AddressError;

    /// Parses what [`Display`](fmt::Display) writes: `sha256:` and 64 lowercase hex digits.
    ///
    /// # Errors
    ///
    /// [`AddressError::MissingPrefix`] where `text` does not start with `sha256:`,
    /// [`AddressError::Length`] where the digits are not 64 bytes long, and
    /// [`AddressError::InvalidDigit`] where one is not `0`-`9` or `a`-`f`.
    fn from_str(text: &str) -> Result<Self, AddressError> {
        let digits = text
            .strip_prefix("sha256:")
            .ok_or(AddressError::MissingPrefix)?;
        if digits.len() != 64 {
            return Err(AddressError::Length(digits.len()));
        }
        let mut digest = [0_u8; 32];
        let (pairs, _) = digits.as_bytes().as_chunks::<2>();
        for (byte, [high, low]) in digest.iter_mut().zip(pairs) {
            *byte = (nibble(*high)? << 4) | nibble(*low)?;
        }
        Ok(Self(digest))
    }
}

fn nibble(digit: u8) -> Result<u8, AddressError> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        other => Err(AddressError::InvalidDigit(other)),
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sha256:")?;
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_sha256_of_the_bytes_with_its_algorithm() {
        assert_eq!(
            Address::of(b"hello world").to_string(),
            "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    const TEXT: &str = "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

    #[test]
    fn parses_what_display_writes() {
        let address = Address::of(b"hello world");
        assert_eq!(address.to_string().parse(), Ok(address));
        assert_eq!(TEXT.parse::<Address>(), Ok(address));
    }

    #[test]
    fn restores_an_address_from_its_digest() {
        let address = Address::of(b"hello world");
        assert_eq!(Address::from_digest(*address.digest()), address);
    }

    #[test]
    fn refuses_an_uppercase_digit() {
        assert_eq!(
            TEXT.to_uppercase()
                .replace("SHA256", "sha256")
                .parse::<Address>(),
            Err(AddressError::InvalidDigit(b'B'))
        );
    }

    #[test]
    fn refuses_a_missing_prefix() {
        assert_eq!(
            TEXT.trim_start_matches("sha256:").parse::<Address>(),
            Err(AddressError::MissingPrefix)
        );
    }

    #[test]
    fn refuses_63_and_65_digits() {
        assert_eq!(
            TEXT[..TEXT.len() - 1].parse::<Address>(),
            Err(AddressError::Length(63))
        );
        assert_eq!(
            format!("{TEXT}0").parse::<Address>(),
            Err(AddressError::Length(65))
        );
    }

    #[test]
    fn refuses_a_non_hex_digit() {
        assert_eq!(
            format!("{}g", &TEXT[..TEXT.len() - 1]).parse::<Address>(),
            Err(AddressError::InvalidDigit(b'g'))
        );
    }
}
