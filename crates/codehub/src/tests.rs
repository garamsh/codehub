//! The shutdown of a serving process.

use std::time::Instant;

use axum::routing::get;
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;

use super::*;

/// How long a test waits for what it expects to happen, so that a regression fails it and does
/// not hang it.
const PATIENCE: Duration = Duration::from_secs(10);

/// A server on a free port whose `/hang` never answers, whose `/slow` answers "done" after 300
/// milliseconds, and whose `/ok` and `/readyz` answer at once, serving until the returned sender
/// sends and for `deadline` after it. The notify fires when `/hang` or `/slow` is entered.
async fn serve_hanging(
    deadline: Duration,
) -> (
    String,
    Arc<Notify>,
    oneshot::Sender<()>,
    JoinHandle<std::io::Result<()>>,
) {
    let entered = Arc::new(Notify::new());
    let (hang, slow) = (Arc::clone(&entered), Arc::clone(&entered));
    let router = axum::Router::new()
        .route(
            "/hang",
            get(move || async move {
                hang.notify_one();
                std::future::pending::<&str>().await
            }),
        )
        .route(
            "/slow",
            get(move || async move {
                slow.notify_one();
                tokio::time::sleep(Duration::from_millis(300)).await;
                "done"
            }),
        )
        .route("/readyz", get(|| async { "ready" }))
        .route("/ok", get(|| async { "ok" }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel::<()>();
    let served = tokio::spawn(serve_until(
        listener,
        router,
        async move {
            let _ = stopped.await;
        },
        deadline,
    ));
    (url, entered, stop, served)
}

#[tokio::test]
async fn ends_shutdown_by_its_deadline_with_a_request_still_open() {
    let (url, entered, stop, served) = serve_hanging(Duration::from_millis(500)).await;
    let open = tokio::spawn(reqwest::get(format!("{url}/hang")));
    entered.notified().await;

    let stopped_at = Instant::now();
    stop.send(()).unwrap();
    let ended = tokio::time::timeout(PATIENCE, served)
        .await
        .expect("shutdown did not end by its deadline");

    assert!(ended.unwrap().is_ok());
    assert!(stopped_at.elapsed() >= Duration::from_millis(400));
    open.abort();
}

#[tokio::test]
async fn ends_shutdown_without_waiting_for_its_deadline_when_no_request_is_open() {
    let (url, _, stop, served) = serve_hanging(Duration::from_secs(30)).await;
    let answered = reqwest::get(format!("{url}/ok")).await.unwrap();
    assert!(answered.status().is_success());
    drop(answered);

    let stopped_at = Instant::now();
    stop.send(()).unwrap();
    let ended = tokio::time::timeout(PATIENCE, served)
        .await
        .expect("shutdown waited for its deadline with nothing open");

    assert!(ended.unwrap().is_ok());
    assert!(stopped_at.elapsed() < Duration::from_secs(20));
}

#[tokio::test]
async fn answers_a_request_in_flight_when_stop_is_signalled_before_the_deadline() {
    let (url, entered, stop, served) = serve_hanging(Duration::from_secs(30)).await;
    let open = tokio::spawn(reqwest::get(format!("{url}/slow")));
    entered.notified().await;

    let stopped_at = Instant::now();
    stop.send(()).unwrap();
    let answered = tokio::time::timeout(PATIENCE, open)
        .await
        .expect("the request in flight was not answered")
        .unwrap()
        .unwrap();

    assert!(answered.status().is_success());
    assert_eq!(answered.text().await.unwrap(), "done");
    let ended = tokio::time::timeout(PATIENCE, served)
        .await
        .expect("shutdown did not end once the request was answered");
    assert!(ended.unwrap().is_ok());
    assert!(stopped_at.elapsed() < Duration::from_secs(15));
}

#[tokio::test]
async fn answers_not_ready_once_stop_is_signalled_while_it_goes_on_serving() {
    let (url, entered, stop, served) = serve_hanging(Duration::from_secs(30)).await;
    let before = reqwest::get(format!("{url}/readyz")).await.unwrap();
    // The control: before the stop the process is ready.
    assert!(before.status().is_success());
    // A request still in flight keeps the drain running while the probe is asked.
    let open = tokio::spawn(reqwest::get(format!("{url}/hang")));
    entered.notified().await;

    stop.send(()).unwrap();
    let after = tokio::time::timeout(PATIENCE, async {
        loop {
            let probe = reqwest::get(format!("{url}/readyz")).await.unwrap();
            if probe.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
                break probe;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("/readyz stayed ready while the server was stopping");

    assert_eq!(after.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    open.abort();
    served.abort();
}
