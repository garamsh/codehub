//! The OIDC adapter against the test identity provider (Dex), and against a stub issuer for the
//! tokens Dex cannot be made to produce. The stub signs with a key the test generates and serves
//! it from a local discovery and key-set endpoint.

use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use adapter_oidc::OidcIdentityVerification;
use app::domain::credential::Credential;
use app::domain::principal::Principal;
use app::port::driven::identity_verification::{IdentityError, IdentityVerification};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
use futures_util::future::join_all;
use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const AUDIENCE: &str = "codehub";
const REFETCH_INTERVAL: Duration = Duration::from_secs(10);

#[cfg(test)]
fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set; run through `make test`"))
}

#[cfg(test)]
fn credential(token: &str) -> Credential {
    Credential::from_str(token).unwrap()
}

#[cfg(test)]
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(test)]
fn dex() -> OidcIdentityVerification {
    OidcIdentityVerification::new(&env("CODEHUB_TEST_OIDC_ISSUER"), AUDIENCE, REFETCH_INTERVAL)
        .unwrap()
}

/// An access token for `user` (`alice`, `bob`, `carol`) from Dex's password grant, with the
/// resource server's audience requested.
#[cfg(test)]
async fn dex_token(user: &str) -> String {
    let issuer = env("CODEHUB_TEST_OIDC_ISSUER");
    let password = env(&format!(
        "CODEHUB_TEST_OIDC_PASSWORD_{}",
        user.to_uppercase()
    ));
    let body = format!(
        "grant_type=password&username={user}%40example.test&password={password}\
         &scope=openid%20audience%3Aserver%3Aclient_id%3Acodehub"
    );
    let response = reqwest::Client::new()
        .post(format!("{issuer}/token"))
        .basic_auth(
            env("CODEHUB_TEST_OIDC_TEST_CLIENT_ID"),
            Some(env("CODEHUB_TEST_OIDC_TEST_CLIENT_SECRET")),
        )
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "Dex refused {user}'s token request"
    );
    let answer: Value = response.json().await.unwrap();
    answer["access_token"].as_str().unwrap().to_owned()
}

/// A signing key and the key id its public half is published under.
#[cfg(test)]
struct TestKey {
    kid: String,
    der: Vec<u8>,
}

#[cfg(test)]
impl TestKey {
    fn generate(kid: &str) -> Self {
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .unwrap();
        Self {
            kid: kid.to_owned(),
            der: pkcs8.as_ref().to_vec(),
        }
    }

    fn jwk(&self) -> Jwk {
        let mut jwk =
            Jwk::from_encoding_key(&EncodingKey::from_ec_der(&self.der), Algorithm::ES256).unwrap();
        jwk.common.key_id = Some(self.kid.clone());
        jwk
    }

    fn sign(&self, claims: &Value) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.kid.clone());
        encode(&header, claims, &EncodingKey::from_ec_der(&self.der)).unwrap()
    }
}

#[cfg(test)]
fn key_set(keys: &[&TestKey]) -> String {
    let set = JwkSet {
        keys: keys.iter().map(|key| key.jwk()).collect(),
    };
    serde_json::to_string(&set).unwrap()
}

/// A local issuer: serves a discovery document and a replaceable key set, and counts key-set
/// requests.
#[cfg(test)]
struct Stub {
    issuer: String,
    jwks: Arc<Mutex<String>>,
    jwks_requests: Arc<AtomicUsize>,
}

#[cfg(test)]
impl Stub {
    /// Starts the stub serving `jwks`; its discovery document names `claimed_issuer` where given.
    async fn start(jwks: String, claimed_issuer: Option<&str>) -> Self {
        Self::start_answering_after(jwks, claimed_issuer, Duration::ZERO).await
    }

    /// As [`Stub::start`], with each key-set request answered `delay` after it arrives.
    async fn start_answering_after(
        jwks: String,
        claimed_issuer: Option<&str>,
        delay: Duration,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let discovery = json!({
            "issuer": claimed_issuer.unwrap_or(&issuer),
            "jwks_uri": format!("{issuer}/keys"),
        })
        .to_string();
        let jwks = Arc::new(Mutex::new(jwks));
        let jwks_requests = Arc::new(AtomicUsize::new(0));
        let (served_jwks, counter) = (Arc::clone(&jwks), Arc::clone(&jwks_requests));
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = vec![0; 4096];
                let read = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(buffer.get(..read).unwrap()).into_owned();
                let body = match request.split_whitespace().nth(1) {
                    Some("/.well-known/openid-configuration") => discovery.clone(),
                    Some("/keys") => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(delay).await;
                        served_jwks.lock().unwrap().clone()
                    }
                    _ => String::new(),
                };
                let status = if body.is_empty() {
                    "404 Not Found"
                } else {
                    "200 OK"
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        Self {
            issuer,
            jwks,
            jwks_requests,
        }
    }

    fn verifier(&self) -> OidcIdentityVerification {
        self.verifier_refetching_after(REFETCH_INTERVAL)
    }

    fn verifier_refetching_after(&self, interval: Duration) -> OidcIdentityVerification {
        OidcIdentityVerification::new(&self.issuer, AUDIENCE, interval).unwrap()
    }

    /// Claims of a token this stub issued for `subject`, valid for an hour.
    fn claims(&self, subject: &str) -> Value {
        json!({
            "iss": self.issuer,
            "sub": subject,
            "aud": [AUDIENCE, "other"],
            "iat": now(),
            "exp": now() + 3600,
        })
    }

    fn requests(&self) -> usize {
        self.jwks_requests.load(Ordering::SeqCst)
    }
}

#[tokio::test]
async fn verifies_a_dex_token_to_the_principal_of_its_user() {
    let verifier = dex();
    let alice = verifier
        .verify(&credential(&dex_token("alice").await))
        .await
        .unwrap();
    let bob = verifier
        .verify(&credential(&dex_token("bob").await))
        .await
        .unwrap();
    let alice_again = verifier
        .verify(&credential(&dex_token("alice").await))
        .await
        .unwrap();

    assert_eq!(alice.issuer(), env("CODEHUB_TEST_OIDC_ISSUER"));
    assert_eq!(alice, alice_again);
    assert_ne!(alice, bob);
}

#[tokio::test]
async fn rejects_a_dex_token_whose_payload_was_swapped_for_another_users() {
    let verifier = dex();
    let alice_token = dex_token("alice").await;
    let bob_token = dex_token("bob").await;
    let mut alice_parts = alice_token.split('.');
    let mut bob_parts = bob_token.split('.');
    let (header, signature) = (alice_parts.next().unwrap(), alice_parts.nth(1).unwrap());
    let payload = bob_parts.nth(1).unwrap();
    let forged = format!("{header}.{payload}.{signature}");

    assert_eq!(
        verifier.verify(&credential(&forged)).await,
        Err(IdentityError::Rejected)
    );
    assert!(verifier.verify(&credential(&alice_token)).await.is_ok());
    assert!(verifier.verify(&credential(&bob_token)).await.is_ok());
}

#[tokio::test]
async fn rejects_a_dex_token_for_another_audience() {
    let token = credential(&dex_token("alice").await);
    let wrong = OidcIdentityVerification::new(
        &env("CODEHUB_TEST_OIDC_ISSUER"),
        "elsewhere",
        REFETCH_INTERVAL,
    )
    .unwrap();

    assert_eq!(wrong.verify(&token).await, Err(IdentityError::Rejected));
    assert!(dex().verify(&token).await.is_ok());
}

#[tokio::test]
async fn rejects_text_that_is_not_a_token() {
    let verifier = dex();
    let unsigned = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIiwia2lkIjoiazEifQ.e30.";

    for garbage in ["not-a-token", "a.b.c", "..", unsigned] {
        assert_eq!(
            verifier.verify(&credential(garbage)).await,
            Err(IdentityError::Rejected),
            "{garbage}"
        );
    }
}

#[tokio::test]
async fn reports_an_unreachable_issuer_as_a_backend_failure() {
    let token = credential(&dex_token("alice").await);
    let unreachable =
        OidcIdentityVerification::new("http://127.0.0.1:1", AUDIENCE, REFETCH_INTERVAL).unwrap();

    assert!(matches!(
        unreachable.verify(&token).await,
        Err(IdentityError::Backend(_))
    ));
}

#[tokio::test]
async fn verifies_a_stub_token_and_rejects_the_same_token_when_one_claim_is_wrong() {
    let key = TestKey::generate("k1");
    let stub = Stub::start(key_set(&[&key]), None).await;
    let verifier = stub.verifier();
    let good = stub.claims("ada");
    let cases = [
        ("expired", json!({"exp": now() - 86_400})),
        ("wrong issuer", json!({"iss": "https://elsewhere.test"})),
        ("wrong audience", json!({"aud": ["elsewhere"]})),
        ("not yet valid", json!({"nbf": now() + 86_400})),
    ];

    let principal = verifier
        .verify(&credential(&key.sign(&good)))
        .await
        .unwrap();
    assert_eq!(principal, Principal::new(&stub.issuer, "ada").unwrap());
    for (name, change) in cases {
        let mut claims = good.clone();
        claims
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        assert_eq!(
            verifier.verify(&credential(&key.sign(&claims))).await,
            Err(IdentityError::Rejected),
            "{name}"
        );
    }
}

#[tokio::test]
async fn rejects_a_stub_token_missing_a_required_claim() {
    let key = TestKey::generate("k1");
    let stub = Stub::start(key_set(&[&key]), None).await;
    let verifier = stub.verifier();
    let good = stub.claims("ada");

    assert!(verifier.verify(&credential(&key.sign(&good))).await.is_ok());
    for claim in ["exp", "iss", "aud", "sub"] {
        let mut claims = good.clone();
        claims.as_object_mut().unwrap().remove(claim);
        assert_eq!(
            verifier.verify(&credential(&key.sign(&claims))).await,
            Err(IdentityError::Rejected),
            "without {claim}"
        );
    }
}

#[tokio::test]
async fn rejects_a_token_signed_by_a_key_the_issuer_did_not_publish() {
    let published = TestKey::generate("k1");
    let stranger = TestKey::generate("k1");
    let stub = Stub::start(key_set(&[&published]), None).await;
    let verifier = stub.verifier();
    let claims = stub.claims("ada");

    assert!(
        verifier
            .verify(&credential(&published.sign(&claims)))
            .await
            .is_ok()
    );
    assert_eq!(
        verifier.verify(&credential(&stranger.sign(&claims))).await,
        Err(IdentityError::Rejected)
    );
}

#[tokio::test]
async fn rejects_an_hmac_token_naming_a_published_hmac_key() {
    let key = TestKey::generate("k1");
    let secret = [0_u8; 24];
    let jwks = json!({"keys": [
        serde_json::to_value(key.jwk()).unwrap(),
        {"kty": "oct", "kid": "hmac", "k": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"},
    ]})
    .to_string();
    let stub = Stub::start(jwks, None).await;
    let verifier = stub.verifier();
    let claims = stub.claims("ada");
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("hmac".to_owned());
    let forged = encode(&header, &claims, &EncodingKey::from_secret(&secret)).unwrap();

    assert!(
        verifier
            .verify(&credential(&key.sign(&claims)))
            .await
            .is_ok()
    );
    assert_eq!(
        verifier.verify(&credential(&forged)).await,
        Err(IdentityError::Rejected)
    );
}

#[tokio::test]
async fn rejects_a_token_that_names_no_key_id() {
    let key = TestKey::generate("k1");
    let stub = Stub::start(key_set(&[&key]), None).await;
    let verifier = stub.verifier();
    let claims = stub.claims("ada");
    let anonymous = encode(
        &Header::new(Algorithm::ES256),
        &claims,
        &EncodingKey::from_ec_der(&key.der),
    )
    .unwrap();

    assert_eq!(
        verifier.verify(&credential(&anonymous)).await,
        Err(IdentityError::Rejected)
    );
    assert_eq!(
        stub.requests(),
        0,
        "a token naming no key must not cause a fetch"
    );
    assert!(
        verifier
            .verify(&credential(&key.sign(&claims)))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn caches_the_key_set_between_verifications() {
    let key = TestKey::generate("k1");
    let stub = Stub::start(key_set(&[&key]), None).await;
    let verifier = stub.verifier();
    let token = credential(&key.sign(&stub.claims("ada")));

    verifier.verify(&token).await.unwrap();
    verifier.verify(&token).await.unwrap();

    assert_eq!(stub.requests(), 1);
}

#[tokio::test]
async fn fetches_the_key_set_again_when_a_token_names_an_unknown_key_id() {
    let old = TestKey::generate("old");
    let new = TestKey::generate("new");
    let stub = Stub::start(key_set(&[&old]), None).await;
    let verifier = stub.verifier_refetching_after(Duration::ZERO);
    let claims = stub.claims("ada");
    verifier
        .verify(&credential(&old.sign(&claims)))
        .await
        .unwrap();
    assert_eq!(
        verifier.verify(&credential(&new.sign(&claims))).await,
        Err(IdentityError::Rejected)
    );

    *stub.jwks.lock().unwrap() = key_set(&[&old, &new]);
    let requests_before = stub.requests();

    assert!(
        verifier
            .verify(&credential(&new.sign(&claims)))
            .await
            .is_ok()
    );
    assert_eq!(stub.requests(), requests_before + 1);
    assert!(
        verifier
            .verify(&credential(&old.sign(&claims)))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn fetches_the_key_set_once_per_interval_however_many_unknown_key_ids_arrive() {
    let key = TestKey::generate("k1");
    let one = TestKey::generate("unknown-1");
    let two = TestKey::generate("unknown-2");
    let stub = Stub::start(key_set(&[&key]), None).await;
    let verifier = stub.verifier();
    let claims = stub.claims("ada");
    verifier
        .verify(&credential(&key.sign(&claims)))
        .await
        .unwrap();
    assert_eq!(stub.requests(), 1);

    for stranger in [&one, &two, &one] {
        assert_eq!(
            verifier.verify(&credential(&stranger.sign(&claims))).await,
            Err(IdentityError::Rejected)
        );
    }

    assert_eq!(stub.requests(), 1);
}

#[tokio::test]
async fn reports_a_discovery_document_for_another_issuer_as_a_backend_failure() {
    let key = TestKey::generate("k1");
    let honest = Stub::start(key_set(&[&key]), None).await;
    let lying = Stub::start(key_set(&[&key]), Some("https://elsewhere.test")).await;
    let token = credential(&key.sign(&honest.claims("ada")));
    let lying_token = credential(&key.sign(&lying.claims("ada")));

    assert!(honest.verifier().verify(&token).await.is_ok());
    assert!(matches!(
        lying.verifier().verify(&lying_token).await,
        Err(IdentityError::Backend(_))
    ));
}

#[tokio::test]
async fn verifies_every_token_that_arrives_during_the_first_key_set_fetch() {
    let key = TestKey::generate("k1");
    let stub =
        Stub::start_answering_after(key_set(&[&key]), None, Duration::from_millis(300)).await;
    let verifier = stub.verifier();
    let token = credential(&key.sign(&stub.claims("ada")));

    let results = join_all((0..8).map(|_| verifier.verify(&token))).await;

    for result in results {
        assert_eq!(result, Ok(Principal::new(&stub.issuer, "ada").unwrap()));
    }
    assert_eq!(stub.requests(), 1);
}

#[tokio::test]
async fn rejects_an_unknown_key_id_that_arrives_during_a_fetch_without_fetching_again() {
    let key = TestKey::generate("k1");
    let stranger = TestKey::generate("unknown");
    let stub =
        Stub::start_answering_after(key_set(&[&key]), None, Duration::from_millis(300)).await;
    let verifier = stub.verifier();
    let claims = stub.claims("ada");
    let known = credential(&key.sign(&claims));
    let unknown = credential(&stranger.sign(&claims));

    let (waited, refused) = tokio::join!(verifier.verify(&known), verifier.verify(&unknown));

    assert!(waited.is_ok());
    assert_eq!(refused, Err(IdentityError::Rejected));
    assert_eq!(stub.requests(), 1);
}

#[tokio::test]
async fn holds_the_refetch_interval_after_a_failed_fetch() {
    let key = TestKey::generate("k1");
    let stub = Stub::start(String::new(), None).await;
    let token = credential(&key.sign(&stub.claims("ada")));
    let throttled = stub.verifier();
    let unthrottled = stub.verifier_refetching_after(Duration::ZERO);

    assert!(matches!(
        throttled.verify(&token).await,
        Err(IdentityError::Backend(_))
    ));
    assert_eq!(throttled.verify(&token).await, Err(IdentityError::Rejected));
    assert_eq!(stub.requests(), 1);

    assert!(matches!(
        unthrottled.verify(&token).await,
        Err(IdentityError::Backend(_))
    ));
    assert!(matches!(
        unthrottled.verify(&token).await,
        Err(IdentityError::Backend(_))
    ));
    assert_eq!(stub.requests(), 3);
}
