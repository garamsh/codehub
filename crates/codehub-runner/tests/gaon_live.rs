//! The gaon driver against a real gaon API server under RBAC, as a ServiceAccount token bound to
//! ADR 0046's ClusterRole. `make e2e-gaon` (dev/gaon/run.sh) builds and starts the server and sets
//! the variables these tests read; without them every test is ignored, so `make ci` runs none.
//!
//! The server has no scheduler and no node agent, so the tests play both as the administrator:
//! they bind a MicroVM and write its phase, and post the result a MicroVM would post.

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use codehub_runner::config::{JobUser, RUNNER_LABEL};
use codehub_runner::executor::{End, Executor, Stop};
use codehub_runner::gaon::GaonExecutor;
use codehub_runner::gaon::api::{Api, Kind};
use codehub_runner::gaon::results::Results;
use codehub_runner::job_source::{Address, Job, JobId, StopReason};
use codehub_runner::token::TokenSource;
use serde_json::{Value, json};
use tokio::sync::OnceCell;

const API_VERSION: &str = "compute.gaon.garam.sh/v1alpha1";
const IMAGE: &str = "registry.invalid/codehub-runner@sha256:0000000000000000000000000000000000000000000000000000000000000000";
const SERVICE_ACCOUNT: &str = "codehub-runner";

/// What the harness exports.
#[cfg(test)]
struct Harness {
    root: String,
    issuer: String,
    ca: PathBuf,
    admin_pem: PathBuf,
    dir: PathBuf,
}

#[cfg(test)]
fn harness() -> Harness {
    let var = |name: &str| {
        std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set; run `make e2e-gaon`"))
    };
    Harness {
        root: var("CODEHUB_GAON_E2E_ROOT"),
        issuer: var("CODEHUB_GAON_E2E_ISSUER"),
        ca: var("CODEHUB_GAON_E2E_CA").into(),
        admin_pem: var("CODEHUB_GAON_E2E_ADMIN_PEM").into(),
        dir: var("CODEHUB_GAON_E2E_DIR").into(),
    }
}

/// A client that trusts the harness's certificate authority, and presents the administrator's
/// certificate where `identity` is given.
#[cfg(test)]
fn client(harness: &Harness, identity: bool) -> reqwest::Client {
    let ca = std::fs::read(&harness.ca).unwrap();
    let mut builder = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(&ca).unwrap());
    if identity {
        let pem = std::fs::read(&harness.admin_pem).unwrap();
        builder = builder.identity(reqwest::Identity::from_pem(&pem).unwrap());
    }
    builder.build().unwrap()
}

/// The administrator, who may do anything.
#[cfg(test)]
struct Admin {
    http: reqwest::Client,
    root: String,
}

#[cfg(test)]
impl Admin {
    fn new() -> Self {
        let harness = harness();
        Self {
            http: client(&harness, true),
            root: harness.root,
        }
    }

    /// The status and the body of one request.
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> (u16, Value) {
        let mut request = self.http.request(method, format!("{}{path}", self.root));
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    /// The body of a request that must have succeeded.
    async fn ok(&self, method: reqwest::Method, path: &str, body: Option<&Value>) -> Value {
        let (status, answer) = self.call(method, path, body).await;
        assert!(
            (200..300).contains(&status),
            "{path} answered {status}: {answer}"
        );
        answer
    }

    async fn get(&self, path: &str) -> Value {
        self.ok(reqwest::Method::GET, path, None).await
    }

    /// The names of the objects of `kind` that carry the label `runner`.
    async fn labelled(&self, kind: &str, runner: &str) -> Vec<String> {
        let list = self
            .get(&format!("/{kind}?labelSelector={RUNNER_LABEL}%3D{runner}"))
            .await;
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["metadata"]["name"].as_str().unwrap().to_owned())
            .collect()
    }

    /// A Secret of `name`, labelled as `labels` says.
    async fn put_secret(&self, name: &str, labels: &Value) -> Value {
        self.ok(
            reqwest::Method::PUT,
            &format!("/secrets/{name}"),
            Some(&secret_body(name, labels)),
        )
        .await
    }

    /// A MicroVM of `name` that runs nothing, labelled as `labels` says.
    async fn put_microvm(&self, name: &str, labels: &Value) -> Value {
        self.ok(
            reqwest::Method::PUT,
            &format!("/microvms/{name}"),
            Some(&json!({
                "apiVersion": API_VERSION,
                "kind": "MicroVM",
                "metadata": { "name": name, "labels": labels },
                "spec": { "image": IMAGE, "runStrategy": "Once" },
            })),
        )
        .await
    }

    /// Waits until `path` exists, and returns it.
    async fn appears(&self, path: &str) -> Value {
        for _ in 0..100 {
            let (status, body) = self.call(reqwest::Method::GET, path, None).await;
            if status == 200 {
                return body;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{path} did not appear");
    }
}

#[cfg(test)]
fn secret_body(name: &str, labels: &Value) -> Value {
    json!({
        "apiVersion": API_VERSION,
        "kind": "Secret",
        "metadata": { "name": name, "labels": labels },
        "data": { "k": STANDARD.encode("v") },
    })
}

/// The token file of the ServiceAccount, written once by the first test to need it.
#[cfg(test)]
async fn token_file() -> PathBuf {
    static TOKEN: OnceCell<PathBuf> = OnceCell::const_new();
    TOKEN
        .get_or_init(|| async {
            let admin = Admin::new();
            let issuer = harness().issuer;
            admin
                .ok(
                    reqwest::Method::PUT,
                    &format!("/serviceaccounts/{SERVICE_ACCOUNT}"),
                    Some(&json!({
                        "apiVersion": API_VERSION,
                        "kind": "ServiceAccount",
                        "metadata": { "name": SERVICE_ACCOUNT },
                    })),
                )
                .await;
            // A token is bound to a MicroVM of its ServiceAccount; this one runs nothing.
            let placeholder = admin
                .ok(
                    reqwest::Method::PUT,
                    "/microvms/codehub-gaon-e2e-token-holder",
                    Some(&json!({
                        "apiVersion": API_VERSION,
                        "kind": "MicroVM",
                        "metadata": { "name": "codehub-gaon-e2e-token-holder" },
                        "spec": {
                            "image": IMAGE,
                            "runStrategy": "Once",
                            "serviceAccountName": SERVICE_ACCOUNT,
                        },
                    })),
                )
                .await;
            assert_eq!(
                placeholder["spec"]["serviceAccountName"], SERVICE_ACCOUNT,
                "setup: the token holder is stored under the ServiceAccount"
            );
            let issued = admin
                .ok(
                    reqwest::Method::POST,
                    &format!("/serviceaccounts/{SERVICE_ACCOUNT}/token"),
                    Some(&json!({
                        "apiVersion": API_VERSION,
                        "kind": "TokenRequest",
                        "spec": {
                            "audiences": [issuer],
                            "expirationSeconds": 3600,
                            "boundMicroVM": {
                                "name": placeholder["metadata"]["name"],
                                "uid": placeholder["metadata"]["uid"],
                            },
                        },
                    })),
                )
                .await;
            let token = issued["status"]["token"].as_str().unwrap();
            let path = harness().dir.join("runner.token");
            std::fs::write(&path, format!("{token}\n")).unwrap();
            path
        })
        .await
        .clone()
}

/// The driver's client under the ServiceAccount's token, for the runner named `runner`.
#[cfg(test)]
async fn runner_api(runner: &str) -> Api {
    let harness = harness();
    Api::new(
        client(&harness, false),
        &harness.root,
        TokenSource::File(token_file().await),
        runner.to_owned(),
    )
}

#[cfg(test)]
fn job(id: u64) -> Job {
    Job {
        id: JobId::new(id),
        organisation: "acme".into(),
        repository: 42,
        changeset: Address::new([9; 32]),
        check: "unit".into(),
        folder: String::new(),
        outputs: Vec::new(),
        caches: Vec::new(),
        command: "true".into(),
        heartbeat_interval: Duration::from_secs(5),
        files: Vec::new(),
    }
}

/// Serves `results` on a local port and returns its base URL.
#[cfg(test)]
async fn listen(results: &Results) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = results.router();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
}

#[cfg(test)]
async fn executor(runner: &str, results: &Results) -> GaonExecutor {
    let base = listen(results).await;
    GaonExecutor::new(
        runner_api(runner).await,
        IMAGE.to_owned(),
        "10001:10001".parse::<JobUser>().unwrap(),
        &base,
        results.clone(),
    )
}

/// A version the server gives an object; they grow with every write.
#[cfg(test)]
fn version(object: &Value) -> u64 {
    object["metadata"]["resourceVersion"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test]
#[ignore = "needs the gaon-apiserver of `make e2e-gaon`"]
async fn a_succeeded_microvm_ends_the_job_with_the_result_posted_to_the_listener() {
    let admin = Admin::new();
    let results = Results::default();
    let executor = executor("live-succeeds", &results).await;
    let (_signal, stop) = Stop::channel();
    let task = tokio::spawn(async move { executor.run(&job(9301), stop).await });

    let secret = admin.appears("/secrets/codehub-job-9301").await;
    let microvm = admin.appears("/microvms/codehub-job-9301").await;
    assert!(
        version(&secret) < version(&microvm),
        "the Secret was created before the MicroVM"
    );
    assert_eq!(
        secret["metadata"]["labels"][RUNNER_LABEL], "live-succeeds",
        "setup: the Secret carries the runner's label"
    );
    let description: Value = serde_json::from_slice(
        &STANDARD
            .decode(secret["data"]["job.json"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();

    // The MicroVM is bound and then reports Succeeded, as a node agent would.
    let bound = admin
        .ok(
            reqwest::Method::POST,
            "/microvms/codehub-job-9301/binding",
            Some(&json!({
                "resourceVersion": microvm["metadata"]["resourceVersion"],
                "nodeName": "codehub-gaon-e2e-node",
                "bindingEpoch": 1,
            })),
        )
        .await;
    let mut done = bound.clone();
    done["status"] = json!({ "phase": "Succeeded", "bindingEpoch": bound["spec"]["bindingEpoch"] });
    admin
        .ok(
            reqwest::Method::PUT,
            "/microvms/codehub-job-9301/status",
            Some(&done),
        )
        .await;

    let posted = admin
        .http
        .post(description["result_url"].as_str().unwrap())
        .bearer_auth(description["result_secret"].as_str().unwrap())
        .json(&json!({ "end": { "exited": 0 }, "output": "from the live job\n" }))
        .send()
        .await
        .unwrap();
    assert_eq!(posted.status().as_u16(), 200);

    let execution = tokio::time::timeout(Duration::from_secs(90), task)
        .await
        .expect("the job ended")
        .unwrap()
        .unwrap();
    assert_eq!(execution.end, End::Exited(0));
    assert_eq!(execution.output, "from the live job\n");
    assert!(admin.labelled("microvms", "live-succeeds").await.is_empty());
    assert!(admin.labelled("secrets", "live-succeeds").await.is_empty());
}

#[tokio::test]
#[ignore = "needs the gaon-apiserver of `make e2e-gaon`"]
async fn a_timeout_deletes_the_microvm_and_the_secret_and_a_labelled_list_shows_them_gone() {
    let admin = Admin::new();
    let results = Results::default();
    let executor = executor("live-times-out", &results).await;
    let (signal, stop) = Stop::channel();
    let task = tokio::spawn(async move { executor.run(&job(9302), stop).await });

    admin.appears("/microvms/codehub-job-9302").await;
    assert_eq!(
        admin.labelled("secrets", "live-times-out").await,
        ["codehub-job-9302"],
        "setup: the job's Secret is listed under the runner's label"
    );
    assert_eq!(
        admin.labelled("microvms", "live-times-out").await,
        ["codehub-job-9302"],
        "setup: the job's MicroVM is listed under the runner's label"
    );
    signal.send(StopReason::TimedOut);

    let execution = tokio::time::timeout(Duration::from_secs(90), task)
        .await
        .expect("the job ended")
        .unwrap()
        .unwrap();
    assert_eq!(execution.end, End::Stopped(StopReason::TimedOut));
    assert!(
        admin
            .labelled("microvms", "live-times-out")
            .await
            .is_empty()
    );
    assert!(admin.labelled("secrets", "live-times-out").await.is_empty());
}

#[tokio::test]
#[ignore = "needs the gaon-apiserver of `make e2e-gaon`"]
async fn start_deletes_the_leftovers_of_its_label_and_no_other_runner_s() {
    let admin = Admin::new();
    let mine = json!({ RUNNER_LABEL: "live-cleans" });
    let other = json!({ RUNNER_LABEL: "live-cleans-other" });
    admin.put_secret("left-over", &mine).await;
    admin.put_microvm("left-over", &mine).await;
    admin.put_secret("someone-else", &other).await;
    admin.put_microvm("someone-else", &other).await;
    assert_eq!(
        admin.labelled("secrets", "live-cleans").await,
        ["left-over"]
    );
    assert_eq!(
        admin.labelled("microvms", "live-cleans").await,
        ["left-over"]
    );

    let executor = executor("live-cleans", &Results::default()).await;
    executor.clean().await.unwrap();

    assert!(admin.labelled("secrets", "live-cleans").await.is_empty());
    assert!(admin.labelled("microvms", "live-cleans").await.is_empty());
    assert_eq!(
        admin.labelled("secrets", "live-cleans-other").await,
        ["someone-else"],
        "control: another runner's objects stay"
    );
    assert_eq!(
        admin.labelled("microvms", "live-cleans-other").await,
        ["someone-else"],
        "control: another runner's objects stay"
    );
}

#[tokio::test]
#[ignore = "needs the gaon-apiserver of `make e2e-gaon`"]
async fn the_service_account_is_refused_objects_without_the_runner_label() {
    let api = runner_api("live-refused").await;
    let admin = Admin::new();
    let bare = secret_body("unlabelled", &json!({}));

    let refused = api.create(Kind::Secret, "unlabelled", &bare).await;
    assert!(
        refused.is_err_and(|error| error.is_status(403)),
        "a Secret without the label is Forbidden"
    );
    let microvm_refused = api
        .create(
            Kind::MicroVm,
            "unlabelled",
            &json!({
                "apiVersion": API_VERSION,
                "kind": "MicroVM",
                "metadata": { "name": "unlabelled" },
                "spec": { "image": IMAGE, "runStrategy": "Once" },
            }),
        )
        .await;
    assert!(
        microvm_refused.is_err_and(|error| error.is_status(403)),
        "a MicroVM without the label is Forbidden"
    );
    let (status, _) = admin
        .call(reqwest::Method::GET, "/secrets/unlabelled", None)
        .await;
    assert_eq!(status, 404, "nothing was stored by the refused creates");

    // Control: the same identity may create the same Secret with the label.
    api.create(
        Kind::Secret,
        "labelled",
        &secret_body("labelled", &api.labels()),
    )
    .await
    .unwrap();
    assert_eq!(
        admin.labelled("secrets", "live-refused").await,
        ["labelled"]
    );
}
