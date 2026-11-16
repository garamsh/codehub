//! The settings of the image registry's token endpoint (ADR 0053), and the signer they build.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use adapter_registry_jwt::{Es256RegistryTokenIssuance, InvalidKey};
use app::port::driven::registry_token_issuance::RegistryTokenIssuance;
use clap::Args;

/// The settings of the token endpoint. It is on where all four are given, and answers 404 where
/// none is; some but not all is a mistake and stops the server.
#[derive(Args, Clone, Debug, Default)]
pub struct RegistryOptions {
    /// The `iss` of the tokens, which the registry's `auth.token.issuer` names.
    #[arg(long, env = "CODEHUB_REGISTRY_ISSUER")]
    pub registry_issuer: Option<String>,
    /// The `aud` of the tokens, the registry's `auth.token.service`; a request for another
    /// service is refused.
    #[arg(long, env = "CODEHUB_REGISTRY_SERVICE")]
    pub registry_service: Option<String>,
    /// The file holding the PEM PKCS#8 ES256 (P-256) private key the tokens are signed with.
    #[arg(long, env = "CODEHUB_REGISTRY_KEY_FILE")]
    pub registry_key_file: Option<PathBuf>,
    /// The `kid` of the key, which the registry's JWKS names it by.
    #[arg(long, env = "CODEHUB_REGISTRY_KEY_ID")]
    pub registry_key_id: Option<String>,
}

/// Why the token endpoint could not be configured.
#[derive(Debug)]
pub enum RegistryError {
    /// Some of the four settings are given and this one is not.
    Missing(&'static str),
    /// This setting is given and empty.
    Empty(&'static str),
    /// The key file could not be read.
    KeyFile(PathBuf, std::io::Error),
    /// The key file does not hold an ES256 private key.
    Key(PathBuf, InvalidKey),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(setting) => write!(
                f,
                "the registry token endpoint needs all of CODEHUB_REGISTRY_ISSUER, \
                 CODEHUB_REGISTRY_SERVICE, CODEHUB_REGISTRY_KEY_FILE and CODEHUB_REGISTRY_KEY_ID \
                 or none, and {setting} is missing"
            ),
            Self::Empty(setting) => write!(f, "{setting} is empty"),
            Self::KeyFile(path, error) => write!(
                f,
                "reading the key file {} named by CODEHUB_REGISTRY_KEY_FILE failed: {error}",
                path.display()
            ),
            Self::Key(path, error) => write!(f, "the key file {}: {error}", path.display()),
        }
    }
}

impl std::error::Error for RegistryError {}

/// The token endpoint as configured: the registry it serves and what signs its tokens.
pub(crate) struct Configured {
    pub(crate) service: String,
    pub(crate) issuance: Arc<dyn RegistryTokenIssuance>,
}

impl RegistryOptions {
    /// The token endpoint, or `None` where none of the settings is given.
    ///
    /// # Errors
    ///
    /// [`RegistryError`] where only some settings are given or the key cannot be read.
    pub(crate) fn configure(&self) -> Result<Option<Configured>, RegistryError> {
        let Self {
            registry_issuer,
            registry_service,
            registry_key_file,
            registry_key_id,
        } = self;
        if registry_issuer.is_none()
            && registry_service.is_none()
            && registry_key_file.is_none()
            && registry_key_id.is_none()
        {
            return Ok(None);
        }
        let text = |value: &Option<String>, setting: &'static str| match value.as_deref() {
            None => Err(RegistryError::Missing(setting)),
            Some("") => Err(RegistryError::Empty(setting)),
            Some(value) => Ok(value.to_owned()),
        };
        let issuer = text(registry_issuer, "CODEHUB_REGISTRY_ISSUER")?;
        let service = text(registry_service, "CODEHUB_REGISTRY_SERVICE")?;
        let key_id = text(registry_key_id, "CODEHUB_REGISTRY_KEY_ID")?;
        let path = registry_key_file
            .clone()
            .ok_or(RegistryError::Missing("CODEHUB_REGISTRY_KEY_FILE"))?;
        let pem =
            std::fs::read(&path).map_err(|error| RegistryError::KeyFile(path.clone(), error))?;
        let issuance = Es256RegistryTokenIssuance::new(&pem, key_id, issuer)
            .map_err(|error| RegistryError::Key(path, error))?;
        Ok(Some(Configured {
            service,
            issuance: Arc::new(issuance),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(
        issuer: Option<&str>,
        service: Option<&str>,
        file: Option<&str>,
        key_id: Option<&str>,
    ) -> RegistryOptions {
        RegistryOptions {
            registry_issuer: issuer.map(str::to_owned),
            registry_service: service.map(str::to_owned),
            registry_key_file: file.map(PathBuf::from),
            registry_key_id: key_id.map(str::to_owned),
        }
    }

    #[test]
    fn is_off_where_no_setting_is_given_and_refuses_where_only_some_are() {
        assert!(
            options(None, None, None, None)
                .configure()
                .unwrap()
                .is_none()
        );
        for partial in [
            options(Some("i"), None, None, None),
            options(Some("i"), Some("s"), Some("/nowhere"), None),
            options(None, Some("s"), Some("/nowhere"), Some("k")),
        ] {
            assert!(matches!(
                partial.configure().err(),
                Some(RegistryError::Missing(_))
            ));
        }
    }

    #[test]
    fn refuses_an_empty_issuer_service_or_key_id_and_takes_it_filled() {
        let file = Some("Cargo.toml");
        for (empty, name) in [
            (options(Some(""), Some("s"), file, Some("k")), "ISSUER"),
            (options(Some("i"), Some(""), file, Some("k")), "SERVICE"),
            (options(Some("i"), Some("s"), file, Some("")), "KEY_ID"),
        ] {
            assert!(
                matches!(empty.configure().err(), Some(RegistryError::Empty(setting)) if setting.ends_with(name))
            );
        }
        // Control: filled, the same settings get as far as reading the key.
        let filled = options(Some("i"), Some("s"), file, Some("k"));
        assert!(matches!(
            filled.configure().err(),
            Some(RegistryError::Key(..))
        ));
    }

    #[test]
    fn names_the_key_file_it_cannot_read_and_the_one_that_is_no_key() {
        let missing = options(Some("i"), Some("s"), Some("/nowhere/key.pem"), Some("k"));
        assert!(matches!(
            missing.configure().err(),
            Some(RegistryError::KeyFile(..))
        ));
        let not_a_key = options(Some("i"), Some("s"), Some("Cargo.toml"), Some("k"));
        assert!(matches!(
            not_a_key.configure().err(),
            Some(RegistryError::Key(..))
        ));
    }
}
