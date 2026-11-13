//! garam's stack (`dev/garam`, started by `make e2e-garam`) as the end-to-end tests meet it: its
//! organisations and people, the tokens it issues, and the policies that make its decisions.
//!
//! garam's console and its platform write what these helpers write directly: a person's sign-in,
//! and the role links and entities of its policy service. Each such helper names that in its
//! documentation, so a reader sees what the test stands in for and what it exercises.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use test_support::{TestDatabase, env};

use super::Server;

/// How long a request, a command or a wait for garam may take.
const BOUND: Duration = Duration::from_secs(60);

/// The audience of garam's tokens that codehub accepts.
const AUDIENCE: &str = "codehub";

/// Sent with every call to garam's api.
const CONTRACT: (&str, &str) = ("Garam-Contract-Version", "operation-authority.v1");

/// The running stack, as the variables of `dev/garam/run.sh` name it.
pub struct Garam {
    http: reqwest::Client,
    issuer: String,
    dir: PathBuf,
    compose: PathBuf,
}

/// A person garam holds: the owner of one organisation, and the tokens garam issues them.
pub struct Person {
    /// The person's GRN, which is the `sub` of their tokens.
    pub grn: String,
    /// The GRN of the organisation they own.
    pub org_grn: String,
    /// The organisation's name in garam, and so in codehub.
    pub org: String,
    /// A token for codehub's audience.
    pub token: String,
    /// A token of the same person for the audience of garam's policy service, which codehub does
    /// not accept.
    pub other_audience_token: String,
}

impl Garam {
    /// Connects to the stack `dev/garam/run.sh` started.
    ///
    /// # Panics
    ///
    /// Panics if the variables the run script sets are missing, or the certificate authority
    /// cannot be read.
    pub fn running() -> Self {
        let ca = std::fs::read(env("CODEHUB_GARAM_E2E_CA")).unwrap();
        let http = reqwest::Client::builder()
            .add_root_certificate(reqwest::Certificate::from_pem(&ca).unwrap())
            .timeout(BOUND)
            .build()
            .unwrap();
        Self {
            http,
            issuer: env("CODEHUB_GARAM_E2E_ISSUER"),
            dir: PathBuf::from(env("CODEHUB_GARAM_E2E_DIR")),
            compose: PathBuf::from(env("CODEHUB_GARAM_E2E_COMPOSE")),
        }
    }

    /// A key file the run script wrote: `codehub.pem`, which garam holds the public half of, or
    /// `other.pem`, which it does not.
    pub fn key(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Starts the built `codehub` with garam as its issuer and decision point, authenticating to
    /// garam with the private key in `key` for the organisation's service principal.
    ///
    /// # Panics
    ///
    /// Panics where `Server::start` does.
    pub fn server(&self, database: &TestDatabase, key: &Path) -> Server {
        self.server_at(database, key, &env("CODEHUB_GARAM_E2E_POLICY_URL"))
    }

    /// Starts `codehub` as [`Garam::server`] does, asking the decision point at `policy`.
    fn server_at(&self, database: &TestDatabase, key: &Path, policy: &str) -> Server {
        let kid = std::fs::read_to_string(self.key("codehub.kid")).unwrap();
        Server::start_with(
            database,
            vec![
                ("CODEHUB_OIDC_ISSUER", self.issuer.clone()),
                ("CODEHUB_OIDC_AUDIENCE", AUDIENCE.to_owned()),
                ("CODEHUB_AUTHORISATION", "authzen".to_owned()),
                ("CODEHUB_AUTHZEN_URL", policy.to_owned()),
                (
                    "CODEHUB_AUTHZEN_TOKEN_ENDPOINT",
                    env("CODEHUB_GARAM_E2E_TOKEN_ENDPOINT"),
                ),
                ("CODEHUB_AUTHZEN_PROFILE", "grn".to_owned()),
                (
                    "CODEHUB_AUTHZEN_CLIENT_AUTHENTICATION",
                    "private-key-jwt".to_owned(),
                ),
                ("CODEHUB_AUTHZEN_KEY_FILE", key.display().to_string()),
                ("CODEHUB_AUTHZEN_KEY_ID", kid),
                ("CODEHUB_AUTHZEN_ISSUER", self.issuer.clone()),
                ("CODEHUB_AUTHZEN_AUDIENCE", "policy".to_owned()),
                (
                    "CODEHUB_AUTHZEN_CLIENT_ID",
                    "grn:{organisation}/iam/service/codehub".to_owned(),
                ),
                ("SSL_CERT_FILE", env("CODEHUB_GARAM_E2E_CA")),
            ],
        )
    }

    /// Starts the built `codehub` as [`Garam::server`] does, with a decision point nothing listens
    /// at.
    ///
    /// # Panics
    ///
    /// Panics where `Server::start` does.
    pub fn server_without_decision_point(&self, database: &TestDatabase) -> Server {
        self.server_at(database, &self.key("codehub.pem"), "https://localhost:9")
    }

    /// Runs `sql` in garam's database `database` (`garam` or `policy`) and returns its output.
    ///
    /// # Panics
    ///
    /// Panics if `docker compose` or the statement fails.
    pub fn psql(&self, database: &str, sql: &str) -> String {
        let service = if database == "policy" {
            "policy-postgres"
        } else {
            "postgres"
        };
        let out = Command::new("docker")
            .args(["compose", "-f"])
            .arg(&self.compose)
            .args(["exec", "-T", service, "psql", "-U", "garam", "-d", database])
            .args(["-v", "ON_ERROR_STOP=1", "-tAc", sql])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "psql: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// Creates an organisation owned by a new person, and returns the person with their tokens.
    ///
    /// The person's sign-in is written as garam's console would write it: a user row and a
    /// session whose cookie is the opaque token this call holds. The organisation is created
    /// through garam's api as the person, and the tokens come from garam's device grant, approved
    /// as the person's session, and its RFC 8693 token exchange.
    ///
    /// # Panics
    ///
    /// Panics if garam refuses a step or the platform's delivery does not finish in time.
    pub async fn person(&self) -> Person {
        let suffix = rand_hex(6);
        let grn = format!("grn:root/iam/user/e2e-{suffix}");
        let cookie = rand_hex(32);
        let hash = hex_sha256(cookie.as_bytes());
        self.psql(
            "garam",
            &format!(
                "INSERT INTO users (grn, nickname, created_at, updated_at) \
                 VALUES ('{grn}', 'e2e-{suffix}', now(), now()); \
                 INSERT INTO sessions (token_hash, user_grn, created_at, expires_at) \
                 VALUES ('{hash}', '{grn}', now(), now() + interval '1 hour')"
            ),
        );
        let session = format!("garam_session={cookie}");

        let org: Value = self
            .api(self.http.post(format!("{}/orgs", self.issuer)))
            .header("Cookie", &session)
            .json(&json!({"name": format!("e2e {suffix}")}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let org_grn = org["grn"].as_str().unwrap().to_owned();
        let name = org_grn.rsplit('/').next().unwrap().to_owned();
        self.wait_for_delivery();

        let device: Value = self
            .http
            .post(format!("{}/auth/device", self.issuer))
            .form(&[("client_id", "garam-cli")])
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let approved = self
            .api(
                self.http
                    .post(format!("{}/auth/device/approval", self.issuer)),
            )
            .header("Cookie", &session)
            .json(&json!({"userCode": device["user_code"], "approve": true}))
            .send()
            .await
            .unwrap();
        assert_eq!(approved.status(), 204, "garam refused the approval");
        tokio::time::sleep(Duration::from_secs(
            device["interval"].as_u64().unwrap_or(1),
        ))
        .await;
        let session_token = self
            .token(
                "/auth/device/token",
                &[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", "garam-cli"),
                    ("device_code", device["device_code"].as_str().unwrap()),
                ],
            )
            .await;
        Person {
            token: self.exchange(&session_token, AUDIENCE).await,
            other_audience_token: self.exchange(&session_token, "policy").await,
            grn,
            org_grn,
            org: name,
        }
    }

    /// Makes `person` the owner of their organisation in garam's policy service.
    ///
    /// garam's platform writes this role link when it creates an organisation; this stand-in
    /// writes it through the policy service's own `CreatePolicy` and `ActivatePolicy`, as the
    /// platform, signing the call as the platform does.
    ///
    /// # Panics
    ///
    /// Panics if the policy service does not answer both calls with gRPC status 0.
    pub fn make_owner(&self, person: &Person) {
        let principal = [string(1, "Iam::User"), string(2, &person.grn)].concat();
        let resource = [string(1, "Iam::Org"), string(2, &person.org_grn)].concat();
        let role_link = [
            varint(1, 1),
            varint(2, 1),
            message(3, &principal),
            message(4, &resource),
        ]
        .concat();
        self.policy(&person.org, "owner", &message(1, &role_link));
    }

    /// Forbids every write under `src` in `person`'s organisation, and nothing else.
    ///
    /// A guardrail the platform's console would write; written here through `CreatePolicy` and
    /// `ActivatePolicy` as the platform. The text is garam's own test of the path context.
    ///
    /// # Panics
    ///
    /// Panics where [`Garam::make_owner`] does.
    pub fn forbid_writes_under_src(&self, person: &Person) {
        let guardrail = r#"forbid ( principal, action == Codehub::Action::"repo.write", resource ) when { context has path && (context.path == "src" || context.path like "src/*") };"#;
        self.policy(&person.org, "no-src", &string(2, guardrail));
    }

    /// Creates and activates the policy `name` with `body` in the organisation `org`.
    fn policy(&self, org: &str, name: &str, body: &[u8]) {
        let id = format!("grn:{org}/iam/policy/codehub-e2e-{name}");
        let org_grn = format!("grn:root/iam/org/{org}");
        let create = [
            string(2, &org_grn),
            string(3, &id),
            string(4, &format!("codehub-e2e-{name}")),
            message(5, body),
        ]
        .concat();
        self.grpc("CreatePolicy", &create);
        self.grpc(
            "ActivatePolicy",
            &[string(2, &org_grn), string(3, &id)].concat(),
        );
    }

    /// One gRPC call to the policy service as the platform, which the service accepts for a JWT
    /// signed with the key whose public half is the issuer's.
    ///
    /// The call is made with `curl`, because the status arrives in a trailer.
    fn grpc(&self, method: &str, message: &[u8]) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let jwks: Value =
            serde_json::from_slice(&std::fs::read(self.dir.join("issuer-jwks.json")).unwrap())
                .unwrap();
        let header = URL_SAFE_NO_PAD.encode(
            json!({"alg": "RS256", "kid": jwks["keys"][0]["kid"], "typ": "JWT"}).to_string(),
        );
        let claims = URL_SAFE_NO_PAD.encode(
            json!({
                "iss": self.issuer, "aud": "policy", "sub": "platform",
                "iat": now, "exp": now + 120, "jti": rand_hex(8)
            })
            .to_string(),
        );
        let signing_input = format!("{header}.{claims}");
        let signature = sign(&self.key("signing.pem"), signing_input.as_bytes());
        let jwt = format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature));

        let mut frame = vec![0];
        frame.extend(u32::try_from(message.len()).unwrap().to_be_bytes());
        frame.extend(message);
        let call = rand_hex(6);
        let scratch = self.dir.join(format!("frame-{call}"));
        std::fs::write(&scratch, frame).unwrap();
        let trailers = self.dir.join(format!("trailers-{call}"));
        let url = format!(
            "{}/garam.policy.v1.PolicyService/{method}",
            env("CODEHUB_GARAM_E2E_POLICY_URL")
        );
        let out = Command::new("curl")
            .args(["-sS", "--http2", "--cacert"])
            .arg(env("CODEHUB_GARAM_E2E_CA"))
            .args(["-H", "content-type: application/grpc", "-H", "te: trailers"])
            .args(["-H", &format!("authorization: Bearer {jwt}")])
            .arg("--data-binary")
            .arg(format!("@{}", scratch.display()))
            .arg("-D")
            .arg(&trailers)
            .args(["-o", "/dev/null", &url])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "the policy service refused {method}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let headers = std::fs::read_to_string(&trailers).unwrap();
        let status = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("grpc-status:")
                    .map(|s| s.trim().to_owned())
            })
            .unwrap_or_default();
        assert_eq!(
            status, "0",
            "{method} answered gRPC status {status}:\n{headers}"
        );
    }

    /// Waits until garam's platform has delivered every entity it holds to the policy service.
    fn wait_for_delivery(&self) {
        let deadline = Instant::now() + BOUND;
        while self.psql("garam", "SELECT count(*) FROM policy_outbox") != "0" {
            assert!(
                Instant::now() < deadline,
                "the outbox still held entities after the bound"
            );
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    /// `request` with the header garam's api requires.
    fn api(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request.header(CONTRACT.0, CONTRACT.1)
    }

    /// The access token of a form post to garam's issuer at `path`.
    async fn token(&self, path: &str, form: &[(&str, &str)]) -> String {
        let response = self
            .http
            .post(format!("{}{path}", self.issuer))
            .form(form)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success(), "garam refused {path}");
        let body: Value = response.json().await.unwrap();
        body["access_token"].as_str().unwrap().to_owned()
    }

    /// A token of the same person for `audience`, by RFC 8693 token exchange.
    async fn exchange(&self, subject_token: &str, audience: &str) -> String {
        self.token(
            "/oauth/token",
            &[
                (
                    "grant_type",
                    "urn:ietf:params:oauth:grant-type:token-exchange",
                ),
                ("subject_token", subject_token),
                (
                    "subject_token_type",
                    "urn:ietf:params:oauth:token-type:access_token",
                ),
                ("audience", audience),
            ],
        )
        .await
    }
}

/// `n` random bytes in hex.
fn rand_hex(n: usize) -> String {
    let out = Command::new("openssl")
        .args(["rand", "-hex", &n.to_string()])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// The SHA-256 of `bytes` in hex.
fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, b| {
            write!(out, "{b:02x}").unwrap();
            out
        })
}

/// The RS256 signature of `input` by the private key in `key`.
fn sign(key: &Path, input: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut child = Command::new("openssl")
        .args(["dgst", "-sha256", "-sign"])
        .arg(key)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "openssl could not sign");
    out.stdout
}

/// A protobuf varint.
fn encode_varint(mut n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while n >= 0x80 {
        out.push(u8::try_from(n & 0x7f).unwrap() | 0x80);
        n >>= 7;
    }
    out.push(u8::try_from(n).unwrap());
    out
}

/// Field `number` as a varint.
fn varint(number: u64, value: u64) -> Vec<u8> {
    [encode_varint(number << 3), encode_varint(value)].concat()
}

/// Field `number` as an embedded message or bytes.
fn message(number: u64, body: &[u8]) -> Vec<u8> {
    [
        encode_varint(number << 3 | 2),
        encode_varint(body.len() as u64),
        body.to_vec(),
    ]
    .concat()
}

/// Field `number` as a string.
fn string(number: u64, value: &str) -> Vec<u8> {
    message(number, value.as_bytes())
}
