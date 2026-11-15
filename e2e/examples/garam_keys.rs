//! Writes the throwaway key material of the garam stack into the directory given as the argument,
//! every file readable by the owner alone: garam's token signing key, the key codehub signs its
//! client assertions with, a second key no service principal holds, the service keys file that
//! registers codehub's public key with garam, the key-encryption key of the machine listener, and a
//! certificate authority with the certificate both TLS listeners serve. Run by `make e2e-garam`;
//! nothing here is committed or printed.

use std::error::Error;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use aws_lc_rs::rand;
use aws_lc_rs::signature::{KeyPair as _, RsaKeyPair};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, PKCS_RSA_SHA256, RsaKeySize, SanType,
};
use serde_json::json;
use sha2::{Digest, Sha256};

/// The public exponent 65537 as base64url, the `e` of every key generated here.
const EXPONENT: &str = "AQAB";

/// The lifetime of the certificates.
const VALIDITY: Duration = Duration::from_secs(2 * 24 * 60 * 60);

fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::args()
        .nth(1)
        .ok_or("the directory to write the keys into is the one argument")?;
    let dir = Path::new(&dir);

    for name in ["signing", "other"] {
        let key = KeyPair::generate_rsa_for(&PKCS_RSA_SHA256, RsaKeySize::_2048)?;
        write(dir, &format!("{name}.pem"), &key.serialize_pem())?;
    }

    let codehub = KeyPair::generate_rsa_for(&PKCS_RSA_SHA256, RsaKeySize::_2048)?;
    write(dir, "codehub.pem", &codehub.serialize_pem())?;
    let modulus = RsaKeyPair::from_pkcs8(&codehub.serialize_der())?
        .public_key()
        .modulus()
        .big_endian_without_leading_zero()
        .to_vec();
    let kid = thumbprint(&modulus);
    write(dir, "codehub.kid", &kid)?;
    let service_keys = json!({"codehub": {"keys": [{
        "kty": "RSA", "use": "sig", "alg": "RS256", "kid": kid,
        "n": URL_SAFE_NO_PAD.encode(&modulus), "e": EXPONENT,
    }]}});
    write(dir, "service-keys.json", &format!("{service_keys}\n"))?;

    let mut kek = [0u8; 32];
    rand::fill(&mut kek)?;
    let kek: String = kek.iter().map(|byte| format!("{byte:02x}")).collect();
    write(dir, "kek.hex", &format!("{kek}\n"))?;

    let now = SystemTime::now();
    let not_after = now + VALIDITY;

    let ca_key = KeyPair::generate()?;
    let mut ca = CertificateParams::default();
    ca.distinguished_name = common_name("codehub garam e2e");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    ca.not_before = now.into();
    ca.not_after = not_after.into();
    let ca_cert = ca.self_signed(&ca_key)?;
    write(dir, "ca.crt", &ca_cert.pem())?;
    write(dir, "ca.key", &ca_key.serialize_pem())?;

    let tls_key = KeyPair::generate()?;
    let mut tls = CertificateParams::default();
    tls.distinguished_name = common_name("garam");
    tls.is_ca = IsCa::ExplicitNoCa;
    tls.subject_alt_names = vec![
        SanType::DnsName("machine".try_into()?),
        SanType::DnsName("policy".try_into()?),
        SanType::DnsName("localhost".try_into()?),
        SanType::IpAddress([127, 0, 0, 1].into()),
    ];
    tls.not_before = now.into();
    tls.not_after = not_after.into();
    let tls_cert = tls.signed_by(&tls_key, &Issuer::new(ca, ca_key))?;
    write(dir, "tls.crt", &tls_cert.pem())?;
    write(dir, "tls.key", &tls_key.serialize_pem())?;
    Ok(())
}

fn common_name(name: &str) -> DistinguishedName {
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, name);
    dn
}

/// Creates `name` in `dir`, readable by the owner alone, holding `contents`.
fn write(dir: &Path, name: &str, contents: &str) -> Result<(), Box<dyn Error>> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join(name))?;
    file.write_all(contents.as_bytes())?;
    Ok(())
}

/// The RFC 7638 thumbprint of the RSA key with public exponent 65537 and the big-endian `modulus`:
/// the SHA-256 of its required members in lexicographic order, as base64url.
fn thumbprint(modulus: &[u8]) -> String {
    let jwk = format!(
        r#"{{"e":"{EXPONENT}","kty":"RSA","n":"{}"}}"#,
        URL_SAFE_NO_PAD.encode(modulus)
    );
    URL_SAFE_NO_PAD.encode(Sha256::digest(jwk))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key of RFC 7638 section 3.1 and the thumbprint the RFC publishes for it.
    const RFC_MODULUS: &str = "0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw";
    const RFC_THUMBPRINT: &str = "NzbLsXh8uDCcd-6MNwXF4W_7noWXFZAfHkxZsRGC9Xs";

    #[test]
    fn thumbprint_matches_the_rfc_7638_example() {
        let modulus = URL_SAFE_NO_PAD.decode(RFC_MODULUS).unwrap();
        assert_eq!(thumbprint(&modulus), RFC_THUMBPRINT);
    }
}
