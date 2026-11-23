//! A client that pins a CA (ADR 0059), against token endpoints served over TLS on the loopback
//! address with certificates generated for each test: no network is involved.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use oauth_client::{Authentication, PinnedCa, TokenClient};
use test_support::tls::{Authority, Leaf, TlsServer};

/// The variable that tells the child process of the system-roots test where its files are.
const CHILD_DIR: &str = "CODEHUB_TEST_PINNED_CA_DIR";

const TOKEN: &str = r#"{"access_token":"issued","token_type":"Bearer","expires_in":3600}"#;

#[cfg(test)]
async fn token_endpoint(leaf: &Leaf) -> TlsServer {
    TlsServer::start(leaf, |_| (200, TOKEN.to_owned())).await
}

/// Whether a client trusting only `pinned`, or the system roots where none, gets a token from
/// `server`.
#[cfg(test)]
async fn reaches(server: &TlsServer, pinned: Option<&str>) -> bool {
    let ca = pinned.map(|pem| PinnedCa::from_pem(pem.as_bytes()).unwrap());
    let client = TokenClient::new(
        &format!("{}/token", server.url),
        None,
        Authentication::ClientSecret {
            client_id: "codehub".to_owned(),
            client_secret: "secret".to_owned(),
        },
        ca.as_ref(),
        Duration::from_secs(10),
    )
    .unwrap();
    client
        .token("acme")
        .await
        .is_ok_and(|token| token.as_str() == "issued")
}

#[tokio::test]
async fn a_pinned_ca_is_reached_and_another_is_refused() {
    let pinned = Authority::new("pinned");
    let other = Authority::new("other");
    let server = token_endpoint(&pinned.leaf()).await;

    // The control: the CA that signed the server's certificate reaches it.
    assert!(reaches(&server, Some(&pinned.pem())).await);
    assert_eq!(server.answered(), 1);
    // Another CA does not, and no request got through.
    assert!(!reaches(&server, Some(&other.pem())).await);
    assert_eq!(server.answered(), 1);
}

#[tokio::test]
async fn the_system_roots_do_not_trust_a_generated_ca() {
    let authority = Authority::new("generated");
    let server = token_endpoint(&authority.leaf()).await;

    assert!(!reaches(&server, None).await);
    assert_eq!(server.answered(), 0);
}

#[tokio::test]
async fn a_bundle_of_two_cas_reaches_a_server_signed_by_either() {
    let first = Authority::new("first");
    let second = Authority::new("second");
    let stranger = Authority::new("stranger");
    let by_first = token_endpoint(&first.leaf()).await;
    let by_second = token_endpoint(&second.leaf()).await;
    let by_stranger = token_endpoint(&stranger.leaf()).await;
    let bundle = format!("{}\n{}", first.pem(), second.pem());

    assert!(reaches(&by_first, Some(&bundle)).await);
    assert!(reaches(&by_second, Some(&bundle)).await);
    // The bundle trusts those two and nothing else.
    assert!(!reaches(&by_stranger, Some(&bundle)).await);
    // The controls: each file of one certificate reaches only its own server.
    assert!(reaches(&by_first, Some(&first.pem())).await);
    assert!(!reaches(&by_second, Some(&first.pem())).await);
}

#[test]
fn a_file_without_a_certificate_is_refused() {
    for pem in ["", "not a certificate\n"] {
        let refused = PinnedCa::from_pem(pem.as_bytes()).err();
        assert!(
            refused.is_some_and(|error| error.to_string().contains("no certificate")),
            "{pem:?}"
        );
    }
}

/// A CA the system trusts is refused by a client that pins another. The system roots of a process
/// come from `SSL_CERT_FILE` at its start, so the check runs in a child process of this binary
/// that is started with it.
#[test]
fn a_system_trusted_ca_is_refused_where_another_is_pinned() {
    let dir = std::env::temp_dir().join(test_support::unique("pinned-ca"));
    std::fs::create_dir(&dir).unwrap();
    let trusted = Authority::new("system-trusted");
    let leaf = trusted.leaf();
    std::fs::write(dir.join("ca.pem"), trusted.pem()).unwrap();
    std::fs::write(dir.join("leaf.crt"), &leaf.certificate).unwrap();
    std::fs::write(dir.join("leaf.key"), &leaf.key).unwrap();
    std::fs::write(dir.join("other.pem"), Authority::new("other").pem()).unwrap();

    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "system_trusted_ca_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_DIR, &dir)
        .env("SSL_CERT_FILE", dir.join("ca.pem"))
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && report.contains("1 passed"),
        "{report}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// What the parent test starts: it does nothing where run on its own.
#[tokio::test]
async fn system_trusted_ca_child() {
    let Some(dir) = std::env::var_os(CHILD_DIR) else {
        return;
    };
    let dir = Path::new(&dir);
    let leaf = Leaf {
        certificate: std::fs::read(dir.join("leaf.crt")).unwrap(),
        key: std::fs::read(dir.join("leaf.key")).unwrap(),
    };
    let other = std::fs::read_to_string(dir.join("other.pem")).unwrap();
    let server = token_endpoint(&leaf).await;

    // The control: with no pin the server is reached, so its CA is trusted by the system.
    assert!(reaches(&server, None).await);
    assert_eq!(server.answered(), 1);
    // Pinning another CA refuses it all the same.
    assert!(!reaches(&server, Some(&other)).await);
    assert_eq!(server.answered(), 1);
}
