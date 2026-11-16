//! Registry token issuance as an ES256 JWT, the token an image registry running the Docker
//! registry token authentication accepts (ADR 0053).

use std::fmt;

use app::port::driven::registry_token_issuance::{
    GrantedEntry, IssuedToken, RegistryTokenIssuance, TokenIssuanceError,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Serialize;

/// The signing key is not a PEM PKCS#8 private key on P-256.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InvalidKey;

impl fmt::Display for InvalidKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the registry signing key is not a PEM PKCS#8 private key on P-256")
    }
}

impl std::error::Error for InvalidKey {}

/// Signs registry tokens with one ES256 key.
pub struct Es256RegistryTokenIssuance {
    key: EncodingKey,
    key_id: String,
    issuer: String,
}

impl fmt::Debug for Es256RegistryTokenIssuance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Es256RegistryTokenIssuance")
            .field("key_id", &self.key_id)
            .field("issuer", &self.issuer)
            .finish_non_exhaustive()
    }
}

impl Es256RegistryTokenIssuance {
    /// A signer with the private key `pem`, naming it `key_id` in each token's header and itself
    /// `issuer` in each token's claims.
    ///
    /// # Errors
    ///
    /// [`InvalidKey`] where `pem` is not a P-256 private key in PEM.
    pub fn new(pem: &[u8], key_id: String, issuer: String) -> Result<Self, InvalidKey> {
        let key = EncodingKey::from_ec_pem(pem).map_err(|_| InvalidKey)?;
        // A key of another curve parses but cannot sign ES256; find that out now.
        jsonwebtoken::encode(&Header::new(Algorithm::ES256), &serde_json::json!({}), &key)
            .map_err(|_| InvalidKey)?;
        Ok(Self {
            key,
            key_id,
            issuer,
        })
    }
}

/// One entry of the `access` claim.
#[derive(Serialize)]
struct Access<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    name: &'a str,
    actions: Vec<&'static str>,
}

#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    iat: i64,
    nbf: i64,
    exp: i64,
    jti: String,
    access: Vec<Access<'a>>,
}

/// A random identifier for one token, 128 bits in hexadecimal.
fn jti() -> Result<String, TokenIssuanceError> {
    let mut bytes = [0_u8; 16];
    aws_lc_rs::rand::fill(&mut bytes)
        .map_err(|_| TokenIssuanceError::Backend("no random bytes for the token".to_owned()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[async_trait::async_trait]
impl RegistryTokenIssuance for Es256RegistryTokenIssuance {
    async fn issue(
        &self,
        subject: &str,
        service: &str,
        entries: &[GrantedEntry],
        issued_at: i64,
        expires_at: i64,
    ) -> Result<IssuedToken, TokenIssuanceError> {
        let claims = Claims {
            iss: &self.issuer,
            sub: subject,
            aud: service,
            iat: issued_at,
            nbf: issued_at,
            exp: expires_at,
            jti: jti()?,
            access: entries
                .iter()
                .map(|entry| Access {
                    kind: &entry.kind,
                    name: &entry.name,
                    actions: entry.actions.iter().map(|action| action.as_str()).collect(),
                })
                .collect(),
        };
        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some("JWT".to_owned());
        header.kid = Some(self.key_id.clone());
        let token = jsonwebtoken::encode(&header, &claims, &self.key)
            .map_err(|error| TokenIssuanceError::Backend(error.to_string()))?;
        Ok(IssuedToken {
            token,
            expires_in: u64::try_from(expires_at - issued_at).unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app::domain::image::ImageAction;
    use jsonwebtoken::{DecodingKey, Validation, decode, decode_header};
    use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384};
    use serde_json::{Value, json};

    fn entry(name: &str, actions: &[ImageAction]) -> GrantedEntry {
        GrantedEntry {
            kind: "repository".to_owned(),
            name: name.to_owned(),
            actions: actions.to_vec(),
        }
    }

    /// A signer and the public key that verifies its tokens.
    fn signer() -> (Es256RegistryTokenIssuance, DecodingKey) {
        let pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let signer = Es256RegistryTokenIssuance::new(
            pair.serialize_pem().as_bytes(),
            "kid-1".to_owned(),
            "codehub.test".to_owned(),
        )
        .unwrap();
        let public = DecodingKey::from_ec_pem(pair.public_key_pem().as_bytes()).unwrap();
        (signer, public)
    }

    fn validation() -> Validation {
        let mut validation = Validation::new(Algorithm::ES256);
        validation.set_audience(&["registry.test"]);
        validation.set_issuer(&["codehub.test"]);
        validation.set_required_spec_claims(&["exp", "aud", "iss", "nbf"]);
        validation.validate_nbf = false;
        validation.validate_exp = false;
        validation
    }

    #[tokio::test]
    async fn signs_a_token_the_public_key_verifies_with_the_claims_it_was_given() {
        let (signer, public) = signer();
        let entries = [
            entry("acme/app/web", &[ImageAction::Pull, ImageAction::Push]),
            entry("acme/app/db", &[]),
        ];

        let issued = signer
            .issue(
                "ada",
                "registry.test",
                &entries,
                1_700_000_000,
                1_700_000_300,
            )
            .await
            .unwrap();

        let header = decode_header(&issued.token).unwrap();
        assert_eq!(header.alg, Algorithm::ES256);
        assert_eq!(header.typ.as_deref(), Some("JWT"));
        assert_eq!(header.kid.as_deref(), Some("kid-1"));
        let claims = decode::<Value>(&issued.token, &public, &validation())
            .unwrap()
            .claims;
        assert_eq!(claims["iss"], "codehub.test");
        assert_eq!(claims["sub"], "ada");
        assert_eq!(claims["aud"], "registry.test");
        assert_eq!(claims["iat"], 1_700_000_000);
        assert_eq!(claims["nbf"], 1_700_000_000);
        assert_eq!(claims["exp"], 1_700_000_300);
        assert_eq!(
            claims["access"],
            json!([
                {"type": "repository", "name": "acme/app/web", "actions": ["pull", "push"]},
                {"type": "repository", "name": "acme/app/db", "actions": []},
            ])
        );
        assert_eq!(issued.expires_in, 300);
    }

    #[tokio::test]
    async fn gives_each_token_its_own_identifier_and_no_other_key_verifies_it() {
        let (signer, public) = signer();
        let (_, stranger) = self::signer();
        let issue = || signer.issue("ada", "registry.test", &[], 0, 300);

        let first = issue().await.unwrap().token;
        let second = issue().await.unwrap().token;

        let jti = |token: &str| {
            decode::<Value>(token, &public, &validation())
                .unwrap()
                .claims["jti"]
                .clone()
        };
        assert_ne!(jti(&first), jti(&second));
        // Control: the key that signed verifies, a stranger's does not.
        assert!(decode::<Value>(&first, &stranger, &validation()).is_err());
    }

    #[test]
    fn refuses_a_key_that_is_not_a_p_256_private_key() {
        // A P-384 key is a real private key in PEM that cannot sign ES256: the probe in `new`
        // refuses it. Text that is no key at all is refused too.
        let other = KeyPair::generate_for(&PKCS_ECDSA_P384_SHA384).unwrap();
        assert_eq!(
            Es256RegistryTokenIssuance::new(
                other.serialize_pem().as_bytes(),
                "k".to_owned(),
                "i".to_owned()
            )
            .err(),
            Some(InvalidKey)
        );
        let text = "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n";
        assert_eq!(
            Es256RegistryTokenIssuance::new(text.as_bytes(), "k".to_owned(), "i".to_owned()).err(),
            Some(InvalidKey)
        );
        let pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        assert!(
            Es256RegistryTokenIssuance::new(
                pair.serialize_pem().as_bytes(),
                "k".to_owned(),
                "i".to_owned()
            )
            .is_ok()
        );
    }
}
