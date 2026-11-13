//! Tests of the gaon executor against a stub of gaon's API. The stub keeps MicroVMs and Secrets,
//! judges requests as a label-selector RBAC rule does, and plays the guest: when a MicroVM is
//! created it reads the Secret the MicroVM references and posts a result to the real listener.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, put};
use axum::{Json, Router};

use crate::config::RUNNER_LABEL;
use crate::job_source::{Address, JobId, StopReason};
use crate::run_job::{Posted, PostedEnd};
use crate::token::TokenSource;

use super::*;

const RUNNER: &str = "runner-a";
const ROOT: &str = "/apis/compute.gaon.garam.sh/v1alpha1";
const IMAGE: &str = "registry.example/codehub-runner@sha256:0000000000000000000000000000000000000000000000000000000000000000";

static NEXT: AtomicU64 = AtomicU64::new(1);

/// What the stub's guest does when its MicroVM is created. A phase is `(name, of the current
/// binding)`; each read of the MicroVM gives the next one, the last repeating.
#[derive(Clone)]
enum Guest {
    Posts {
        posted: Posted,
        phases: Vec<(&'static str, bool)>,
    },
    Dies {
        phase: &'static str,
        reason: &'static str,
    },
    Hangs,
}

struct Stored {
    body: Value,
    version: u64,
    /// Lists that still hold the object, once it is deleted.
    deleting: Option<usize>,
    phases: Vec<(&'static str, bool)>,
    reason: Option<&'static str>,
}

#[derive(Clone, Debug)]
struct Call {
    method: &'static str,
    kind: String,
    name: Option<String>,
    auth: String,
    /// Whether the MicroVM of that name was still held when the call came.
    microvm_held: bool,
    body: Option<Value>,
}

#[derive(Default)]
struct Inner {
    objects: BTreeMap<(String, String), Stored>,
    calls: Vec<Call>,
    next_version: u64,
    lag: usize,
    guest: Option<Guest>,
    forbid_delete_once: bool,
    stale_delete_once: bool,
    fail_puts: bool,
}

#[derive(Clone, Default)]
struct Stub(Arc<Mutex<Inner>>);

impl Stub {
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap()
    }

    fn calls(&self) -> Vec<Call> {
        self.inner().calls.clone()
    }

    fn held(&self) -> Vec<(String, String)> {
        self.inner().objects.keys().cloned().collect()
    }

    /// Stores an object as if another party had created it.
    fn preload(&self, kind: &str, name: &str, runner: Option<&str>) {
        let mut inner = self.inner();
        let labels = runner.map_or_else(|| json!({}), |value| json!({ RUNNER_LABEL: value }));
        inner.next_version += 1;
        let version = inner.next_version;
        inner.objects.insert(
            (kind.to_owned(), name.to_owned()),
            Stored {
                body: json!({ "metadata": { "name": name, "labels": labels }, "spec": {} }),
                version,
                deleting: None,
                phases: vec![("Running", true)],
                reason: None,
            },
        );
    }
}

fn error(code: StatusCode, reason: &str) -> (StatusCode, Json<Value>) {
    (
        code,
        Json(
            json!({ "code": code.as_u16(), "reason": reason, "message": format!("{reason} in detail") }),
        ),
    )
}

fn auth_of(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

fn labelled(body: &Value) -> bool {
    body.pointer("/metadata/labels")
        .and_then(|labels| labels.get(RUNNER_LABEL))
        .and_then(Value::as_str)
        == Some(RUNNER)
}

type Answer = Result<Json<Value>, (StatusCode, Json<Value>)>;

async fn create(
    State(stub): State<Stub>,
    Path((kind, name)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Answer {
    let guest = {
        let mut inner = stub.inner();
        let microvm_held = inner
            .objects
            .contains_key(&("microvms".to_owned(), name.clone()));
        inner.calls.push(Call {
            method: "PUT",
            kind: kind.clone(),
            name: Some(name.clone()),
            auth: auth_of(&headers),
            microvm_held,
            body: Some(body.clone()),
        });
        if inner.fail_puts {
            return Err(error(StatusCode::INTERNAL_SERVER_ERROR, "Internal"));
        }
        if !labelled(&body) {
            return Err(error(StatusCode::FORBIDDEN, "Forbidden"));
        }
        if inner.objects.contains_key(&(kind.clone(), name.clone())) {
            return Err(error(StatusCode::CONFLICT, "AlreadyExists"));
        }
        let mut stored_body = body.clone();
        if kind == "microvms" {
            stored_body["spec"]["bindingEpoch"] = json!(2);
        }
        let (phases, reason, guest) = match (kind.as_str(), inner.guest.clone()) {
            ("microvms", Some(Guest::Posts { posted, phases })) => {
                (phases.clone(), None, Some((posted, phases)))
            }
            ("microvms", Some(Guest::Dies { phase, reason })) => {
                (vec![(phase, true)], Some(reason), None)
            }
            _ => (vec![("Running", true)], None, None),
        };
        inner.next_version += 1;
        let version = inner.next_version;
        inner.objects.insert(
            (kind.clone(), name.clone()),
            Stored {
                body: stored_body,
                version,
                deleting: None,
                phases,
                reason,
            },
        );
        guest.map(|(posted, _)| posted).and_then(|posted| {
            let data = inner
                .objects
                .get(&("secrets".to_owned(), name.clone()))?
                .body
                .pointer("/data/job.json")?
                .as_str()?
                .to_owned();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .ok()?;
            let description: Value = serde_json::from_slice(&bytes).ok()?;
            Some((
                description.get("result_url")?.as_str()?.to_owned(),
                description.get("result_secret")?.as_str()?.to_owned(),
                posted,
            ))
        })
    };
    if let Some((url, secret, posted)) = guest {
        let _ = reqwest::Client::new()
            .post(url)
            .bearer_auth(secret)
            .json(&posted)
            .send()
            .await;
    }
    Ok(Json(body))
}

async fn read(
    State(stub): State<Stub>,
    Path((kind, name)): Path<(String, String)>,
    headers: HeaderMap,
) -> Answer {
    let mut inner = stub.inner();
    inner.calls.push(Call {
        method: "GET",
        kind: kind.clone(),
        name: Some(name.clone()),
        auth: auth_of(&headers),
        microvm_held: false,
        body: None,
    });
    let Some(stored) = inner.objects.get_mut(&(kind, name)) else {
        return Err(error(StatusCode::FORBIDDEN, "Forbidden"));
    };
    if !labelled(&stored.body) {
        return Err(error(StatusCode::FORBIDDEN, "Forbidden"));
    }
    let (phase, current) = if stored.phases.len() > 1 {
        stored.phases.remove(0)
    } else {
        stored.phases.first().copied().unwrap_or(("Pending", true))
    };
    let mut body = stored.body.clone();
    body["status"] = json!({ "phase": phase, "bindingEpoch": if current { 2 } else { 1 } });
    if let Some(reason) = stored.reason {
        body["status"]["conditions"] =
            json!([{ "type": "Ready", "status": "False", "reason": reason, "message": "" }]);
    }
    Ok(Json(body))
}

async fn list(
    State(stub): State<Stub>,
    Path(kind): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Answer {
    let mut inner = stub.inner();
    inner.calls.push(Call {
        method: "LIST",
        kind: kind.clone(),
        name: None,
        auth: auth_of(&headers),
        microvm_held: false,
        body: None,
    });
    if query.get("labelSelector").map(String::as_str) != Some(&format!("{RUNNER_LABEL}={RUNNER}")) {
        return Err(error(StatusCode::FORBIDDEN, "Forbidden"));
    }
    for stored in inner.objects.values_mut() {
        stored.deleting = stored.deleting.map(|left| left.saturating_sub(1));
    }
    inner
        .objects
        .retain(|_, stored| stored.deleting.is_none_or(|left| left > 0));
    // An object deleted this list is gone from the next one; one with lag left is still held.
    let items: Vec<Value> = inner
        .objects
        .iter()
        .filter(|((k, _), stored)| *k == kind && labelled(&stored.body))
        .map(|((_, name), stored)| {
            let mut body = stored.body.clone();
            body["metadata"]["resourceVersion"] = json!(stored.version.to_string());
            if stored.deleting.is_some() {
                body["metadata"]["deletionTimestamp"] = json!("2026-11-13T00:00:00Z");
            }
            body["metadata"]["name"] = json!(name);
            body
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

async fn delete(
    State(stub): State<Stub>,
    Path((kind, name)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Answer {
    let mut inner = stub.inner();
    let microvm_held = inner
        .objects
        .contains_key(&("microvms".to_owned(), name.clone()));
    inner.calls.push(Call {
        method: "DELETE",
        kind: kind.clone(),
        name: Some(name.clone()),
        auth: auth_of(&headers),
        microvm_held,
        body: None,
    });
    let Some(version) = query.get("resourceVersion") else {
        return Err(error(StatusCode::UNPROCESSABLE_ENTITY, "Invalid"));
    };
    let key = (kind, name);
    let lag = inner.lag;
    let held = inner
        .objects
        .get(&key)
        .is_some_and(|stored| labelled(&stored.body));
    if !held {
        return Err(error(StatusCode::FORBIDDEN, "Forbidden"));
    }
    if inner.forbid_delete_once {
        inner.forbid_delete_once = false;
        inner.objects.remove(&key);
        return Err(error(StatusCode::FORBIDDEN, "Forbidden"));
    }
    inner.next_version += 1;
    let bumped = inner.next_version;
    let stale = std::mem::take(&mut inner.stale_delete_once);
    let Some(stored) = inner.objects.get_mut(&key) else {
        return Err(error(StatusCode::FORBIDDEN, "Forbidden"));
    };
    if stale {
        stored.version = bumped;
        return Err(error(StatusCode::CONFLICT, "Conflict"));
    }
    if stored.version.to_string() != *version {
        return Err(error(StatusCode::CONFLICT, "Conflict"));
    }
    stored.deleting = Some(lag);
    Ok(Json(json!({})))
}

struct Rig {
    stub: Stub,
    executor: GaonExecutor,
    token: PathBuf,
    result_url: String,
}

async fn serve(app: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

async fn rig(guest: Guest) -> Rig {
    let stub = Stub::default();
    stub.inner().guest = Some(guest);
    let app = Router::new()
        .route(
            &format!("{ROOT}/{{kind}}/{{name}}"),
            put(create).get(read).delete(delete),
        )
        .route(&format!("{ROOT}/{{kind}}"), get(list))
        .with_state(stub.clone());
    let gaon = serve(app).await;
    let results = Results::default();
    let listener = serve(results.router()).await;
    let dir = std::env::temp_dir().join(format!(
        "runner-gaon-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let token = dir.join("token");
    std::fs::write(&token, "token-one\n").unwrap();
    let api = Api::new(
        reqwest::Client::new(),
        &format!("http://{gaon}{ROOT}"),
        TokenSource::File(token.clone()),
        RUNNER.to_owned(),
    );
    let result_url = format!("http://{listener}");
    let executor = GaonExecutor::new(
        api,
        IMAGE.to_owned(),
        "4242:4343".parse().unwrap(),
        &result_url,
        results,
    )
    .with_timing(
        std::time::Duration::from_millis(10),
        std::time::Duration::from_secs(5),
    );
    Rig {
        stub,
        executor,
        token,
        result_url,
    }
}

fn job(id: u64, command: &str) -> Job {
    Job {
        id: JobId::new(id),
        organisation: "acme".into(),
        repository: 42,
        changeset: Address::new([9; 32]),
        check: "unit".into(),
        command: command.into(),
        heartbeat_interval: Duration::from_secs(5),
        files: Vec::new(),
    }
}

/// Runs the job, failing the test where it does not end: a driver that misses a final phase
/// waits for ever.
async fn run_with(rig: &Rig, job: &Job, stop: Stop) -> Result<Execution, ExecutorError> {
    tokio::time::timeout(Duration::from_secs(10), rig.executor.run(job, stop))
        .await
        .expect("the job ends within 10 seconds")
}

async fn run(rig: &Rig, job: &Job) -> Result<Execution, ExecutorError> {
    let (_signal, stop) = Stop::channel();
    run_with(rig, job, stop).await
}

fn posts(code: i32, output: &str, phases: &[(&'static str, bool)]) -> Guest {
    Guest::Posts {
        posted: Posted {
            end: PostedEnd::Exited(code),
            output: output.to_owned(),
        },
        phases: phases.to_vec(),
    }
}

fn puts(calls: &[Call]) -> Vec<&Call> {
    calls.iter().filter(|call| call.method == "PUT").collect()
}

#[tokio::test]
async fn a_job_creates_a_secret_then_a_microvm_that_carry_the_label_and_reference_the_file() {
    let rig = rig(posts(0, "ok\n", &[("Succeeded", true)])).await;
    run(&rig, &job(1, "make test")).await.unwrap();
    let calls = rig.stub.calls();
    let created = puts(&calls);
    assert_eq!(created.len(), 2);
    let (secret, microvm) = (created[0], created[1]);
    assert_eq!(
        (secret.kind.as_str(), microvm.kind.as_str()),
        ("secrets", "microvms")
    );
    for call in &created {
        assert_eq!(call.name.as_deref(), Some("codehub-job-1"));
        let body = call.body.as_ref().unwrap();
        assert_eq!(body["apiVersion"], "compute.gaon.garam.sh/v1alpha1");
        assert_eq!(body["metadata"]["labels"][RUNNER_LABEL], RUNNER);
    }
    let spec = &microvm.body.as_ref().unwrap()["spec"];
    assert_eq!(spec["runStrategy"], "Once");
    assert_eq!(spec["image"], IMAGE);
    assert_eq!(spec["user"], "4242:4343");
    assert_eq!(
        spec["command"],
        json!([
            "codehub-runner",
            "run-job",
            "--job",
            "/run/codehub-job.json"
        ])
    );
    assert_eq!(spec["files"].as_array().unwrap().len(), 1);
    let file = &spec["files"][0];
    assert_eq!(file["path"], "/run/codehub-job.json");
    assert_eq!(file["mode"], 0o400);
    assert_eq!(
        file["valueFrom"]["secretKeyRef"],
        json!({ "name": "codehub-job-1", "key": "job.json" })
    );
    assert!(file.get("content").is_none(), "the file is by reference");
    let data = secret.body.as_ref().unwrap()["data"]["job.json"]
        .as_str()
        .unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    let description: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(description["command"], "make test");
    assert_eq!(
        description["result_url"],
        format!("{}/results/1", rig.result_url)
    );
    assert_eq!(description["result_secret"].as_str().unwrap().len(), 64);
}

#[tokio::test]
async fn a_succeeded_phase_gives_the_exit_code_and_output_the_job_posted() {
    let phases = [("Pending", true), ("Running", true), ("Succeeded", true)];
    let rig = rig(posts(0, "all passed\n", &phases)).await;
    let execution = run(&rig, &job(1, "true")).await.unwrap();
    assert_eq!(execution.end, End::Exited(0));
    assert_eq!(execution.output, "all passed\n");
}

#[tokio::test]
async fn a_failed_phase_gives_the_exit_code_and_output_the_job_posted() {
    let rig = rig(posts(7, "boom\n", &[("Running", true), ("Failed", true)])).await;
    let execution = run(&rig, &job(1, "false")).await.unwrap();
    assert_eq!(execution.end, End::Exited(7));
    assert_eq!(execution.output, "boom\n");
}

#[tokio::test]
async fn a_status_of_an_earlier_binding_is_not_a_final_phase() {
    let phases = [("Failed", false), ("Succeeded", true)];
    let rig = rig(posts(0, "late\n", &phases)).await;
    let execution = run(&rig, &job(1, "true")).await.unwrap();
    assert_eq!(execution.end, End::Exited(0));
    let reads = rig
        .stub
        .calls()
        .iter()
        .filter(|call| call.method == "GET" && call.kind == "microvms")
        .count();
    assert_eq!(reads, 2, "the stale Failed was read and not taken");
}

#[tokio::test]
async fn a_final_phase_without_a_result_is_an_executor_error_that_names_phase_and_reason() {
    let rig = rig(Guest::Dies {
        phase: "Failed",
        reason: "ReferenceNotFound",
    })
    .await;
    let error = run(&rig, &job(1, "true")).await.unwrap_err();
    assert!(matches!(
        &error,
        ExecutorError::NoResult { phase, reason: Some(reason) }
            if phase == "Failed" && reason == "ReferenceNotFound"
    ));
    let text = error.to_string();
    assert!(
        text.contains("Failed") && text.contains("ReferenceNotFound"),
        "{text}"
    );
    assert!(rig.stub.held().is_empty(), "its objects are removed");
}

#[tokio::test]
async fn a_stop_request_deletes_both_objects_and_ends_the_job_stopped() {
    for reason in [StopReason::Cancelled, StopReason::TimedOut] {
        let rig = rig(Guest::Hangs).await;
        let (signal, stop) = Stop::channel();
        signal.send(reason);
        let execution = run_with(&rig, &job(1, "sleep 999"), stop).await.unwrap();
        assert_eq!(execution.end, End::Stopped(reason));
        let calls = rig.stub.calls();
        assert_eq!(puts(&calls).len(), 2, "the MicroVM existed to be deleted");
        let deleted: Vec<_> = calls
            .iter()
            .filter(|call| call.method == "DELETE")
            .map(|call| call.kind.as_str())
            .collect();
        assert_eq!(deleted, ["microvms", "secrets"]);
        assert!(rig.stub.held().is_empty());
    }
}

#[tokio::test]
async fn after_a_job_the_secret_is_deleted_only_once_the_labelled_list_lacks_the_microvm() {
    let rig = rig(posts(0, "", &[("Succeeded", true)])).await;
    rig.stub.inner().lag = 3;
    run(&rig, &job(1, "true")).await.unwrap();
    let calls = rig.stub.calls();
    let position = |method: &str, kind: &str| {
        calls
            .iter()
            .position(|call| call.method == method && call.kind == kind && call.name.is_some())
    };
    let secret_delete = calls
        .iter()
        .position(|call| call.method == "DELETE" && call.kind == "secrets")
        .unwrap();
    assert!(
        !calls[secret_delete].microvm_held,
        "the MicroVM was still held"
    );
    let microvm_delete = position("DELETE", "microvms").unwrap();
    let lists_between = calls[microvm_delete..secret_delete]
        .iter()
        .filter(|call| call.method == "LIST" && call.kind == "microvms")
        .count();
    assert!(
        lists_between >= 3,
        "{lists_between} lists between the deletes"
    );
    assert!(rig.stub.held().is_empty());
}

#[tokio::test]
async fn a_delete_answered_forbidden_because_the_object_is_gone_is_not_a_failure() {
    let rig = rig(posts(0, "", &[("Succeeded", true)])).await;
    rig.stub.inner().forbid_delete_once = true;
    let execution = run(&rig, &job(1, "true")).await.unwrap();
    assert_eq!(execution.end, End::Exited(0));
    assert!(rig.stub.held().is_empty(), "the Secret was deleted too");
}

#[tokio::test]
async fn a_delete_conflict_is_retried_at_the_version_the_next_list_gives() {
    let rig = rig(posts(0, "", &[("Succeeded", true)])).await;
    rig.stub.inner().stale_delete_once = true;
    run(&rig, &job(1, "true")).await.unwrap();
    let deletes = rig
        .stub
        .calls()
        .iter()
        .filter(|call| call.method == "DELETE" && call.kind == "microvms")
        .count();
    assert_eq!(deletes, 2);
    assert!(rig.stub.held().is_empty());
}

#[tokio::test]
async fn starting_deletes_this_runners_microvms_then_secrets_and_nothing_else() {
    let rig = rig(Guest::Hangs).await;
    rig.stub.preload("microvms", "left-1", Some(RUNNER));
    rig.stub.preload("secrets", "left-1", Some(RUNNER));
    rig.stub.preload("secrets", "left-2", Some(RUNNER));
    rig.stub.preload("microvms", "theirs", Some("runner-b"));
    rig.stub.preload("secrets", "unlabelled", None);
    rig.executor.clean().await.unwrap();
    assert_eq!(
        rig.stub.held(),
        [
            ("microvms".to_owned(), "theirs".to_owned()),
            ("secrets".to_owned(), "unlabelled".to_owned()),
        ]
    );
    let deleted: Vec<_> = rig
        .stub
        .calls()
        .iter()
        .filter(|call| call.method == "DELETE")
        .map(|call| call.kind.clone())
        .collect();
    assert_eq!(deleted, ["microvms", "secrets", "secrets"]);
}

#[tokio::test]
async fn a_description_over_512_kib_is_refused_before_anything_is_created() {
    let rig = rig(posts(0, "", &[("Succeeded", true)])).await;
    let url = format!("{}/results/1", rig.result_url);
    let empty = Description::new(&job(1, ""), url, "0".repeat(64));
    let base = serde_json::to_vec(&empty).unwrap().len() + DESCRIPTION_KEY.len();
    let exact = DESCRIPTION_LIMIT - base;
    let at_limit = run(&rig, &job(1, &"a".repeat(exact))).await.unwrap();
    assert_eq!(at_limit.end, End::Exited(0), "exactly the limit is held");
    let before = rig.stub.calls().len();
    let error = run(&rig, &job(1, &"a".repeat(exact + 1)))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ExecutorError::DescriptionTooLarge { size, limit }
            if size == DESCRIPTION_LIMIT + 1 && limit == DESCRIPTION_LIMIT
    ));
    assert_eq!(rig.stub.calls().len(), before, "gaon was not called");
}

#[tokio::test]
async fn the_token_file_is_read_again_before_each_request() {
    let rig = rig(posts(0, "", &[("Succeeded", true)])).await;
    run(&rig, &job(1, "true")).await.unwrap();
    let first = rig.stub.calls().len();
    std::fs::write(&rig.token, "token-two\n").unwrap();
    run(&rig, &job(2, "true")).await.unwrap();
    let calls = rig.stub.calls();
    assert!(
        calls[..first]
            .iter()
            .all(|call| call.auth == "Bearer token-one")
    );
    assert!(
        calls[first..]
            .iter()
            .all(|call| call.auth == "Bearer token-two")
    );
}

#[tokio::test]
async fn a_refused_request_is_an_executor_error_that_does_not_carry_the_token() {
    let rig = rig(Guest::Hangs).await;
    rig.stub.inner().fail_puts = true;
    let error = run(&rig, &job(1, "true")).await.unwrap_err();
    assert!(matches!(&error, ExecutorError::Orchestrator(reason) if reason.contains("500")));
    assert!(error.to_string().contains("Internal in detail"), "{error}");
    for text in [error.to_string(), format!("{error:?}")] {
        assert!(!text.contains("token-one"), "{text}");
    }
}
