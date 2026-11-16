use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use sha2::Digest as _;

use super::*;

static NEXT: AtomicU64 = AtomicU64::new(1);

type Seen = Arc<Mutex<Vec<(u64, Value)>>>;

/// The address and bytes of each file PUT to a grant.
type Puts = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// A server that never answers the heartbeat of job 7, answers a claim with `claim` (no job
/// where `None`), and answers the heartbeat of job 1 with a stop for time, of job 2 with a stop
/// for cancellation, of job 3 with a continue and of any other job with a 404; that has no
/// job to claim; and that keeps the bodies of the results it is sent. Returns its URL.
async fn server(seen: Seen, claim: Option<Value>) -> String {
    let app = Router::new()
        .route(
            "/v1/runner/jobs/claim",
            post(move || async move {
                claim.map_or_else(
                    || StatusCode::NO_CONTENT.into_response(),
                    |job| Json(job).into_response(),
                )
            }),
        )
        .route(
            "/v1/runner/jobs/{id}/heartbeat",
            post(|Path(id): Path<u64>| async move {
                match id {
                    1 => (
                        StatusCode::OK,
                        Json(json!({"action": "stop", "reason": "timed_out"})),
                    ),
                    2 => (
                        StatusCode::OK,
                        Json(json!({"action": "stop", "reason": "cancelled"})),
                    ),
                    3 => (StatusCode::OK, Json(json!({"action": "continue"}))),
                    7 => std::future::pending().await,
                    _ => (StatusCode::NOT_FOUND, Json(Value::Null)),
                }
            }),
        )
        .route(
            "/v1/runner/jobs/{id}/result",
            post(
                |State(seen): State<Seen>, Path(id): Path<u64>, Json(body): Json<Value>| async move {
                    seen.lock().unwrap().push((id, body));
                    StatusCode::NO_CONTENT
                },
            ),
        )
        .with_state(seen);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    url
}

/// A source of the server, and the token file it reads, which the caller removes.
async fn source(seen: Seen) -> (HttpJobSource, PathBuf) {
    source_claiming(seen, None).await
}

async fn source_claiming(seen: Seen, claim: Option<Value>) -> (HttpJobSource, PathBuf) {
    let url = server(seen, claim).await;
    let file = std::env::temp_dir().join(format!(
        "codehub-runner-http-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::write(&file, "token\n").unwrap();
    (
        HttpJobSource::new(
            reqwest::Client::new(),
            &url,
            TokenSource::File(file.clone()),
        ),
        file,
    )
}

#[tokio::test]
async fn a_heartbeat_answer_is_read_by_its_reason() {
    let (source, file) = source(Seen::default()).await;
    assert!(matches!(
        source.heartbeat(JobId::new(1)).await,
        Ok(Heartbeat::Stop(StopReason::TimedOut))
    ));
    assert!(matches!(
        source.heartbeat(JobId::new(2)).await,
        Ok(Heartbeat::Stop(StopReason::Cancelled))
    ));
    assert!(matches!(
        source.heartbeat(JobId::new(3)).await,
        Ok(Heartbeat::Continue)
    ));
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn a_job_the_server_does_not_hold_is_not_held() {
    let (source, file) = source(Seen::default()).await;
    assert!(matches!(
        source.heartbeat(JobId::new(9)).await,
        Err(SourceError::NotHeld)
    ));
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn no_content_is_no_job() {
    let (source, file) = source(Seen::default()).await;
    assert!(matches!(source.claim().await, Ok(None)));
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn a_report_carries_its_status_exit_code_and_output() {
    let seen = Seen::default();
    let (source, file) = source(seen.clone()).await;
    let failed = Report {
        status: Status::Failed,
        exit_code: Some(7),
        output: "broken".into(),
        produced: Produced::default(),
        objects: None,
        run: None,
    };
    source.report(JobId::new(5), &failed).await.unwrap();
    let timed_out = Report {
        status: Status::TimedOut,
        exit_code: None,
        output: String::new(),
        produced: Produced::default(),
        objects: None,
        run: None,
    };
    source.report(JobId::new(6), &timed_out).await.unwrap();
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![
            (
                5,
                json!({"status": "failed", "exit_code": 7, "output_tail": "broken",
                       "outputs": [], "caches": []})
            ),
            (
                6,
                json!({"status": "timed_out", "output_tail": "", "outputs": [], "caches": []})
            ),
        ]
    );
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn a_heartbeat_the_server_does_not_answer_ends_at_the_call_timeout() {
    let (source, file) = source(Seen::default()).await;
    let source = source.with_call_timeout(Duration::from_millis(200));
    let answer = tokio::time::timeout(Duration::from_secs(10), source.heartbeat(JobId::new(7)))
        .await
        .expect("the heartbeat outlived the call timeout");
    assert!(matches!(answer, Err(SourceError::Transport(_))));
    std::fs::remove_file(file).unwrap();
}

fn claimed(path: &str) -> Value {
    json!({
        "id": 1,
        "organisation": "acme",
        "repository_id": 3,
        "changeset": format!("sha256:{}", "1".repeat(64)),
        "check": {
            "id": "web:unit", "folder": "web", "command": "true", "timeout": 60,
            "outputs": ["web/dist"], "caches": ["web/.cache"]
        },
        "heartbeat_interval": 5,
        "files": [{
            "path": path,
            "address": format!("sha256:{}", "2".repeat(64)),
            "executable": false,
            "download": {"url": "http://files", "headers": []}
        }]
    })
}

#[tokio::test]
async fn a_claimed_file_path_is_parsed_where_it_enters() {
    let (source, file) = source_claiming(Seen::default(), Some(claimed("a/b"))).await;
    let job = source.claim().await.unwrap().unwrap();
    assert_eq!(job.files[0].path.to_string(), "a/b");
    assert_eq!(
        (job.check.as_str(), job.folder.as_str()),
        ("web:unit", "web")
    );
    assert_eq!(job.outputs[0].to_string(), "web/dist");
    assert_eq!(job.caches[0].to_string(), "web/.cache");
    std::fs::remove_file(file).unwrap();
    for bad in ["../escape", "/abs", "", "a/../../b"] {
        let (source, file) = source_claiming(Seen::default(), Some(claimed(bad))).await;
        let answer = source.claim().await;
        assert!(matches!(answer, Err(SourceError::Malformed(_))), "{bad:?}");
        std::fs::remove_file(file).unwrap();
    }
}

/// A server that grants an upload of every address it is asked about except `held`, and keeps
/// the bodies PUT to the grants and the results reported. Returns its URL.
async fn storing(held: Address, puts: Puts, seen: Seen) -> String {
    let url = Arc::new(Mutex::new(String::new()));
    let grants_url = Arc::clone(&url);
    let app = Router::new()
        .route(
            "/v1/runner/jobs/{id}/uploads",
            post(move |Json(body): Json<Value>| {
                let url = grants_url.lock().unwrap().clone();
                async move {
                    let grants: Vec<Value> = body["files"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|file| file["address"] != json!(held.to_string()))
                        .map(|file| {
                            json!({
                                "address": file["address"],
                                "grant": {
                                    "url": format!("{url}/store/{}", file["address"].as_str().unwrap()),
                                    "headers": [{"name": "x-grant", "value": "yes"}]
                                }
                            })
                        })
                        .collect();
                    Json(json!({ "grants": grants }))
                }
            }),
        )
        .route(
            "/store/{address}",
            axum::routing::put(
                |State(puts): State<Puts>,
                 Path(address): Path<String>,
                 headers: axum::http::HeaderMap,
                 body: axum::body::Bytes| async move {
                    assert_eq!(headers["x-grant"], "yes");
                    puts.lock().unwrap().push((address, body.to_vec()));
                    StatusCode::OK
                },
            ),
        )
        .with_state(puts)
        .route(
            "/v1/runner/jobs/{id}/result",
            post(
                |State(seen): State<Seen>, Path(id): Path<u64>, Json(body): Json<Value>| async move {
                    seen.lock().unwrap().push((id, body));
                    StatusCode::NO_CONTENT
                },
            )
            .with_state(seen),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = format!("http://{}", listener.local_addr().unwrap());
    *url.lock().unwrap() = at.clone();
    tokio::spawn(async move { axum::serve(listener, app).await });
    at
}

fn produced_file(path: &str, bytes: &[u8]) -> ProducedFile {
    ProducedFile {
        path: path.parse().unwrap(),
        address: Address::new(sha2::Sha256::digest(bytes).into()),
        executable: false,
        bytes: bytes.to_vec(),
    }
}

#[tokio::test]
async fn a_report_uploads_the_files_the_server_lacks_and_lists_all_of_them() {
    let kept = produced_file("web/dist/a", b"alpha");
    let new = produced_file("web/.cache/b", b"beta");
    let puts = Arc::default();
    let seen = Seen::default();
    let url = storing(kept.address, Arc::clone(&puts), seen.clone()).await;
    let file = std::env::temp_dir().join(format!(
        "codehub-runner-http-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::write(&file, "token\n").unwrap();
    let source = HttpJobSource::new(
        reqwest::Client::new(),
        &url,
        TokenSource::File(file.clone()),
    );
    let report = Report {
        status: Status::Passed,
        exit_code: Some(0),
        output: String::new(),
        produced: Produced {
            outputs: vec![kept.clone()],
            caches: vec![new.clone()],
        },
        objects: None,
        run: None,
    };
    source.report(JobId::new(4), &report).await.unwrap();
    assert_eq!(
        *puts.lock().unwrap(),
        [(new.address.to_string(), b"beta".to_vec())],
        "only the file the server lacks is uploaded"
    );
    let seen = seen.lock().unwrap().clone();
    let [(4, body)] = seen.as_slice() else {
        panic!("{seen:?}")
    };
    assert_eq!(body["outputs"][0]["path"], "web/dist/a");
    assert_eq!(body["caches"][0]["address"], json!(new.address.to_string()));
    std::fs::remove_file(file).unwrap();
}

fn run_job_json() -> Value {
    let grant = json!({"url": "http://files/x", "headers": [{"name": "x-grant", "value": "yes"}]});
    let file = |path: &str| {
        json!({"path": path, "address": format!("sha256:{}", "ab".repeat(32)), "size": 3,
            "executable": true, "download": grant})
    };
    json!({
        "id": 11, "organisation": "acme", "repository_id": 3, "draft": "feature",
        "command": "make", "timeout": 600, "add": ["docs"], "outputs": ["dist"],
        "env": {"CODEHUB_DRAFT": "feature",
            "CODEHUB_CHANGESET": format!("sha256:{}", "cd".repeat(32))},
        "heartbeat_interval": 10,
        "files": [file("src/main.rs")],
        "caches": [{"path": "target", "files": [file("deps/lib")]}],
    })
}

#[tokio::test]
async fn a_claim_that_names_a_draft_is_a_run() {
    let (source, file) = source_claiming(Seen::default(), Some(run_job_json())).await;
    let job = source.claim().await.unwrap().unwrap();
    let run = job.run.unwrap();
    assert_eq!(run.draft, "feature");
    assert_eq!(job.command, "make");
    assert_eq!(job.check, "");
    assert_eq!(job.folder, "");
    assert_eq!(
        job.changeset.to_string(),
        format!("sha256:{}", "cd".repeat(32))
    );
    assert_eq!(run.add[0].to_string(), "docs");
    assert_eq!(job.outputs[0].to_string(), "dist");
    assert_eq!(job.caches[0].to_string(), "target");
    assert_eq!(run.caches[0].files[0].path.to_string(), "deps/lib");
    assert_eq!(job.files[0].path.to_string(), "src/main.rs");
    assert!(
        run.env
            .contains(&("CODEHUB_DRAFT".into(), "feature".into()))
    );
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn a_run_without_a_base_has_the_zero_changeset() {
    let mut wire = run_job_json();
    wire["env"] = json!({"CODEHUB_DRAFT": "feature"});
    let (source, file) = source_claiming(Seen::default(), Some(wire)).await;
    let job = source.claim().await.unwrap().unwrap();
    assert_eq!(job.changeset, Address::new([0; 32]));
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn a_claim_that_names_a_check_is_a_check_job() {
    let wire = json!({
        "id": 2, "organisation": "acme", "repository_id": 3,
        "changeset": format!("sha256:{}", "cd".repeat(32)),
        "check": {"id": "unit", "folder": "", "command": "true", "outputs": [], "caches": []},
        "heartbeat_interval": 10, "files": [],
    });
    let (source, file) = source_claiming(Seen::default(), Some(wire)).await;
    let job = source.claim().await.unwrap().unwrap();
    assert!(job.run.is_none());
    assert_eq!(job.check, "unit");
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn a_claim_states_the_executor_and_without_one_sends_no_body() {
    let bodies: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = Arc::clone(&bodies);
    let app = Router::new().route(
        "/v1/runner/jobs/claim",
        post(move |body: axum::body::Bytes| async move {
            seen.lock()
                .unwrap()
                .push(String::from_utf8(body.to_vec()).unwrap());
            StatusCode::NO_CONTENT
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    let file = std::env::temp_dir().join(format!(
        "codehub-runner-http-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::write(&file, "token\n").unwrap();
    let make = || {
        HttpJobSource::new(
            reqwest::Client::new(),
            &url,
            TokenSource::File(file.clone()),
        )
    };
    make().claim().await.unwrap();
    make()
        .with_executor(ExecutorKind::Host)
        .claim()
        .await
        .unwrap();
    make()
        .with_executor(ExecutorKind::Gaon)
        .claim()
        .await
        .unwrap();
    assert_eq!(
        *bodies.lock().unwrap(),
        ["", r#"{"executor":"host"}"#, r#"{"executor":"gaon"}"#]
    );
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn a_run_report_names_its_streams_changes_and_refusals_and_uploads_each_object_once() {
    let out = Stream::new(b"hello".to_vec(), false);
    let err = Stream::new(b"hello".to_vec(), true);
    let edited = produced_file("a.txt", b"new");
    let mut tool = produced_file("tool", b"#!/bin/sh");
    tool.executable = true;
    let puts = Arc::default();
    let seen = Seen::default();
    let url = storing(
        produced_file("none", b"none").address,
        Arc::clone(&puts),
        seen.clone(),
    )
    .await;
    let file = std::env::temp_dir().join(format!(
        "codehub-runner-http-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::write(&file, "token\n").unwrap();
    let source = HttpJobSource::new(
        reqwest::Client::new(),
        &url,
        TokenSource::File(file.clone()),
    );
    let report = Report {
        status: Status::Failed,
        exit_code: Some(3),
        output: String::new(),
        produced: Produced {
            outputs: vec![edited.clone()],
            caches: Vec::new(),
        },
        objects: Some(ObjectCounts {
            downloaded: 2,
            cached: 5,
        }),
        run: Some(RunOutcome {
            stdout: Some(out.clone()),
            stderr: Some(err.clone()),
            changes: vec![
                Change::Written(edited.clone()),
                Change::Written(tool.clone()),
                Change::Deleted("gone.txt".parse().unwrap()),
            ],
            refused: vec!["link".into()],
        }),
    };
    source.report(JobId::new(8), &report).await.unwrap();
    let mut addresses: Vec<String> = puts
        .lock()
        .unwrap()
        .iter()
        .map(|(a, _)| a.clone())
        .collect();
    addresses.sort();
    let mut wanted = vec![
        out.address.to_string(),
        edited.address.to_string(),
        tool.address.to_string(),
    ];
    wanted.sort();
    assert_eq!(
        addresses, wanted,
        "each object once, though two share a digest"
    );
    let seen = seen.lock().unwrap().clone();
    let [(8, body)] = seen.as_slice() else {
        panic!("{seen:?}")
    };
    assert_eq!(
        *body,
        json!({
            "outcome": "exited", "exit_code": 3,
            "stdout": {"address": out.address.to_string(), "cut": false},
            "stderr": {"address": err.address.to_string(), "cut": true},
            "objects": {"downloaded": 2, "from_cache": 5},
            "outputs": [{"path": "a.txt", "address": edited.address.to_string(), "executable": false}],
            "caches": [],
            "changes": [
                {"path": "a.txt", "kind": "regular", "address": edited.address.to_string()},
                {"path": "tool", "kind": "executable", "address": tool.address.to_string()},
                {"path": "gone.txt", "kind": "deleted", "address": null},
            ],
            "refused": ["link"],
        })
    );
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn a_run_without_a_command_outcome_reports_executor_failed_with_null_streams() {
    let seen = Seen::default();
    let (source, file) = source(seen.clone()).await;
    for (status, exit_code, name) in [
        (Status::Failed, None, "executor_failed"),
        (Status::TimedOut, None, "timed_out"),
        (Status::Cancelled, None, "cancelled"),
    ] {
        let report = Report {
            status,
            exit_code,
            output: String::new(),
            produced: Produced::default(),
            objects: None,
            run: Some(RunOutcome::default()),
        };
        source.report(JobId::new(5), &report).await.unwrap();
        let body = seen.lock().unwrap().pop().unwrap().1;
        assert_eq!(body["outcome"], name);
        assert_eq!(body["exit_code"], Value::Null);
        assert_eq!(body["stdout"], Value::Null);
        assert_eq!(body["objects"], json!({"downloaded": 0, "from_cache": 0}));
    }
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn a_check_report_carries_the_object_counts() {
    let seen = Seen::default();
    let (source, file) = source(seen.clone()).await;
    let report = Report {
        status: Status::Passed,
        exit_code: Some(0),
        output: String::new(),
        produced: Produced::default(),
        objects: Some(ObjectCounts {
            downloaded: 1,
            cached: 2,
        }),
        run: None,
    };
    source.report(JobId::new(6), &report).await.unwrap();
    let body = seen.lock().unwrap().pop().unwrap().1;
    assert_eq!(body["objects"], json!({"downloaded": 1, "from_cache": 2}));
    assert!(body.get("outcome").is_none());
    std::fs::remove_file(file).unwrap();
}

#[tokio::test]
async fn a_run_whose_upload_fails_reports_executor_failed_and_names_no_object() {
    let seen = Seen::default();
    // This server has no uploads route.
    let (source, file) = source(seen.clone()).await;
    let edited = produced_file("a.txt", b"new");
    let report = Report {
        status: Status::Passed,
        exit_code: Some(0),
        output: String::new(),
        produced: Produced {
            outputs: vec![edited.clone()],
            caches: Vec::new(),
        },
        objects: Some(ObjectCounts {
            downloaded: 1,
            cached: 0,
        }),
        run: Some(RunOutcome {
            stdout: Some(Stream::new(b"out".to_vec(), false)),
            changes: vec![Change::Written(edited)],
            ..RunOutcome::default()
        }),
    };
    source.report(JobId::new(9), &report).await.unwrap();
    let body = seen.lock().unwrap().pop().unwrap().1;
    assert_eq!(body["outcome"], "executor_failed");
    assert_eq!(body["exit_code"], Value::Null);
    assert_eq!(body["stdout"], Value::Null);
    assert_eq!(body["outputs"], json!([]));
    assert_eq!(body["changes"], json!([]));
    assert_eq!(body["objects"], json!({"downloaded": 1, "from_cache": 0}));
    std::fs::remove_file(file).unwrap();
}

// The wire structs against the schemas of `crates/adapter-http/openapi.json`.

fn document() -> Value {
    let path = format!(
        "{}/../adapter-http/openapi.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn resolve<'a>(document: &'a Value, schema: &'a Value) -> &'a Value {
    match schema["$ref"].as_str() {
        Some(reference) => resolve(
            document,
            document.pointer(reference.trim_start_matches('#')).unwrap(),
        ),
        None => schema,
    }
}

/// An instance of `schema` that carries every property it names.
fn instance(document: &Value, schema: &Value) -> Value {
    let schema = resolve(document, schema);
    if let Some(variants) = schema["oneOf"].as_array() {
        let first = variants.iter().find(|v| v["type"] != "null").unwrap();
        return instance(document, first);
    }
    if let Some(values) = schema["enum"].as_array() {
        return values[0].clone();
    }
    let kind = match &schema["type"] {
        Value::Array(kinds) => kinds.iter().find(|k| *k != "null").unwrap().clone(),
        kind @ (Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Object(_)) => kind.clone(),
    };
    match kind.as_str().unwrap() {
        "string" if schema["pattern"].is_string() => json!(format!("sha256:{}", "0".repeat(64))),
        "string" => json!("x"),
        "integer" => json!(1),
        "boolean" => json!(true),
        "array" => json!([instance(document, &schema["items"])]),
        "object" if schema["properties"].is_object() => Value::Object(
            schema["properties"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(name, property)| (name.clone(), instance(document, property)))
                .collect(),
        ),
        "object" => json!({"CODEHUB_DRAFT": "x"}),
        other => panic!("{other}"),
    }
}

/// Whether `value` fits `schema`: no property beyond the schema's, every required one present.
fn conforms(document: &Value, value: &Value, schema: &Value) -> Result<(), String> {
    let schema = resolve(document, schema);
    if let Some(variants) = schema["oneOf"].as_array() {
        return variants
            .iter()
            .any(|variant| conforms(document, value, variant).is_ok())
            .then_some(())
            .ok_or_else(|| format!("{value} fits none of {variants:?}"));
    }
    if let Some(values) = schema["enum"].as_array() {
        return values
            .contains(value)
            .then_some(())
            .ok_or_else(|| format!("{value} is not one of {values:?}"));
    }
    let kinds: Vec<&str> = match &schema["type"] {
        Value::Array(kinds) => kinds.iter().filter_map(Value::as_str).collect(),
        Value::String(kind) => vec![kind],
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => Vec::new(),
    };
    let fits = |kind: &str| match kind {
        "null" => value.is_null(),
        "string" => value.is_string(),
        "integer" => value.is_u64() || value.is_i64(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    };
    if !kinds.iter().any(|kind| fits(kind)) {
        return Err(format!("{value} is none of {kinds:?}"));
    }
    if let (Some(items), Some(values)) = (schema.get("items"), value.as_array()) {
        for item in values {
            conforms(document, item, items)?;
        }
    }
    if let (Some(properties), Some(object)) = (schema["properties"].as_object(), value.as_object())
    {
        for name in object.keys() {
            if !properties.contains_key(name) {
                return Err(format!("{name} is not a property"));
            }
        }
        for required in schema["required"].as_array().into_iter().flatten() {
            let name = required.as_str().unwrap();
            if !object.contains_key(name) {
                return Err(format!("{name} is required"));
            }
        }
        for (name, property) in properties {
            if let Some(member) = object.get(name) {
                conforms(document, member, property)?;
            }
        }
    }
    Ok(())
}

fn assert_conforms(value: &impl Serialize, schema: &str) {
    let document = document();
    let schema = &document["components"]["schemas"][schema];
    let value = serde_json::to_value(value).unwrap();
    conforms(&document, &value, schema).unwrap_or_else(|why| panic!("{why} in {value}"));
}

#[test]
fn the_conformance_check_refuses_what_a_schema_does_not_name() {
    let document = document();
    let schema = &document["components"]["schemas"]["ObjectCounts"];
    assert!(
        conforms(
            &document,
            &json!({"downloaded": 1, "from_cache": 2}),
            schema
        )
        .is_ok()
    );
    assert!(conforms(&document, &json!({"downloaded": 1, "cached": 2}), schema).is_err());
    assert!(conforms(&document, &json!({"downloaded": 1}), schema).is_err());
}

#[test]
fn a_claim_is_read_as_the_schema_variant_it_matches() {
    let document = document();
    let schemas = &document["components"]["schemas"];
    let answer = &document["paths"]["/v1/runner/jobs/claim"]["post"]["responses"]["200"]["content"]
        ["application/json"]["schema"]["oneOf"];
    assert_eq!(
        *answer,
        json!([{"$ref": "#/components/schemas/Job"}, {"$ref": "#/components/schemas/RunJob"}])
    );
    let run: WireClaim = serde_json::from_value(instance(&document, &schemas["RunJob"])).unwrap();
    let check: WireClaim = serde_json::from_value(instance(&document, &schemas["Job"])).unwrap();
    assert!(matches!(run, WireClaim::Run(_)));
    assert!(matches!(check, WireClaim::Check(_)));
    let WireClaim::Run(run) = run else { return };
    let job = run.into_job().unwrap();
    let run = job.run.unwrap();
    assert_eq!(run.draft, "x");
    assert_eq!(run.caches[0].files[0].path.to_string(), "x");
    assert!(job.heartbeat_interval > Duration::ZERO);
}

#[test]
fn a_claim_request_names_an_executor_the_schema_lists() {
    let document = document();
    let schema = &document["paths"]["/v1/runner/jobs/claim"]["post"]["requestBody"]["content"]["application/json"]
        ["schema"];
    for executor in ["host", "gaon"] {
        let body = serde_json::to_value(WireClaimRequest { executor }).unwrap();
        conforms(&document, &body, schema).unwrap();
    }
}

#[test]
fn a_run_report_fits_its_schema_in_every_shape() {
    let stream = || {
        Some(WireStream {
            address: format!("sha256:{}", "1".repeat(64)),
            cut: true,
        })
    };
    let produced = || {
        vec![WireProduced {
            path: "a".into(),
            address: format!("sha256:{}", "2".repeat(64)),
            executable: true,
        }]
    };
    let change = |change: &Change| WireChange::of(change);
    let written = ProducedFile {
        path: "a".parse().unwrap(),
        address: Address::new([3; 32]),
        executable: true,
        bytes: Vec::new(),
    };
    for (status, code) in [
        (Status::Passed, Some(0)),
        (Status::Failed, Some(1)),
        (Status::Failed, None),
        (Status::TimedOut, None),
        (Status::Cancelled, None),
    ] {
        for streams in [true, false] {
            assert_conforms(
                &WireRunReport {
                    outcome: outcome_name(status, code),
                    exit_code: code,
                    stdout: if streams { stream() } else { None },
                    stderr: if streams { stream() } else { None },
                    objects: WireObjects::of(Some(ObjectCounts {
                        downloaded: 1,
                        cached: 2,
                    })),
                    outputs: produced(),
                    caches: produced(),
                    changes: vec![
                        change(&Change::Written(written.clone())),
                        change(&Change::Written(ProducedFile {
                            executable: false,
                            ..written.clone()
                        })),
                        change(&Change::Deleted("b".parse().unwrap())),
                    ],
                    refused: vec!["c".into()],
                },
                "RunReport",
            );
        }
    }
}

#[test]
fn a_check_result_fits_its_schema_with_and_without_counts() {
    for status in [
        Status::Passed,
        Status::Failed,
        Status::TimedOut,
        Status::Cancelled,
    ] {
        for (code, objects) in [
            (
                Some(0),
                Some(ObjectCounts {
                    downloaded: 1,
                    cached: 2,
                }),
            ),
            (None, None),
        ] {
            assert_conforms(
                &WireResult {
                    status: status_name(status),
                    exit_code: code,
                    output_tail: "tail",
                    outputs: vec![WireProduced {
                        path: "a".into(),
                        address: format!("sha256:{}", "2".repeat(64)),
                        executable: false,
                    }],
                    caches: Vec::new(),
                    objects: objects.map(|counts| WireObjects::of(Some(counts))),
                },
                "JobResult",
            );
        }
    }
}

#[test]
fn the_upload_request_and_grants_fit_their_schemas() {
    assert_conforms(
        &WireUploads {
            files: vec![WireUpload {
                address: format!("sha256:{}", "4".repeat(64)),
                size: 3,
            }],
        },
        "JobUploads",
    );
    let document = document();
    let grants = instance(
        &document,
        &document["components"]["schemas"]["JobUploadGrants"],
    );
    let parsed: WireUploadGrants = serde_json::from_value(grants).unwrap();
    assert_eq!(parsed.grants.len(), 1);
}
