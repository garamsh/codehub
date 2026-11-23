//! Certificate authorities and a TLS server for the tests of clients that pin a CA, in memory and
//! on the loopback address, so no network is involved.

use std::sync::{Arc, Mutex};

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType, IsCa, Issuer,
    KeyPair, KeyUsagePurpose, SanType,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// A certificate authority generated for one test.
pub struct Authority {
    params: CertificateParams,
    key: KeyPair,
    certificate: Certificate,
}

/// A certificate for `127.0.0.1` and `localhost` and its private key, both DER.
pub struct Leaf {
    /// The certificate.
    pub certificate: Vec<u8>,
    /// The PKCS#8 private key.
    pub key: Vec<u8>,
}

impl Authority {
    /// A new authority named `name`.
    ///
    /// # Panics
    ///
    /// Panics if the key or certificate cannot be generated.
    #[must_use]
    pub fn new(name: &str) -> Self {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::default();
        let mut subject = DistinguishedName::new();
        subject.push(DnType::CommonName, name);
        params.distinguished_name = subject;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let certificate = params.self_signed(&key).unwrap();
        Self {
            params,
            key,
            certificate,
        }
    }

    /// The authority's certificate, PEM.
    #[must_use]
    pub fn pem(&self) -> String {
        self.certificate.pem()
    }

    /// A certificate this authority signs for the loopback address.
    ///
    /// # Panics
    ///
    /// Panics if the key or certificate cannot be generated.
    #[must_use]
    pub fn leaf(&self) -> Leaf {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::default();
        params.is_ca = IsCa::ExplicitNoCa;
        params.subject_alt_names = vec![
            SanType::DnsName("localhost".try_into().unwrap()),
            SanType::IpAddress([127, 0, 0, 1].into()),
        ];
        let certificate = params
            .signed_by(&key, &Issuer::from_params(&self.params, &self.key))
            .unwrap();
        Leaf {
            certificate: certificate.der().to_vec(),
            key: key.serialize_der(),
        }
    }
}

/// A server on the loopback address that answers each request over TLS with what `respond`
/// returns for it, until the test ends.
pub struct TlsServer {
    /// Its base URL, `https://127.0.0.1:<port>`.
    pub url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl TlsServer {
    /// Starts the server with `leaf`; `respond` is given each request's text, headers and body,
    /// and returns the status and the JSON body of the answer.
    ///
    /// # Panics
    ///
    /// Panics if the certificate is refused or the loopback address cannot be bound.
    pub async fn start(
        leaf: &Leaf,
        respond: impl Fn(&str) -> (u16, String) + Send + 'static,
    ) -> Self {
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(leaf.certificate.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf.key.clone())),
            )
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("https://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&requests);
        tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                // A client that refuses the certificate abandons the handshake, which is what
                // the tests of a refusal look for.
                let Ok(mut stream) = acceptor.accept(socket).await else {
                    continue;
                };
                let request = read_request(&mut stream).await;
                let (status, body) = respond(&request);
                kept.lock().unwrap().push(request);
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        Self { url, requests }
    }

    /// How many requests were answered.
    ///
    /// # Panics
    ///
    /// Panics if the record of requests was poisoned by a panic in `respond`.
    #[must_use]
    pub fn answered(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

/// Reads one request: its headers and as many body bytes as its `content-length` says.
async fn read_request<S: AsyncReadExt + Unpin>(stream: &mut S) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await.unwrap();
        buffer.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&buffer).into_owned();
        if let Some((headers, body)) = text.split_once("\r\n\r\n") {
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if body.len() >= length || read == 0 {
                return text;
            }
        } else if read == 0 {
            return text;
        }
    }
}
