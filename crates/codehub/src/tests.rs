//! The shutdown of a serving process.

use std::time::Instant;

use axum::routing::get;
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;

use super::*;

/// A server on a free port whose `/hang` never answers and whose `/ok` does, serving until the
/// returned sender sends and for `deadline` after it. The notify fires when `/hang` is entered.
async fn serve_hanging(
    deadline: Duration,
) -> (
    String,
    Arc<Notify>,
    oneshot::Sender<()>,
    JoinHandle<std::io::Result<()>>,
) {
    let entered = Arc::new(Notify::new());
    let hang = Arc::clone(&entered);
    let router = axum::Router::new()
        .route(
            "/hang",
            get(move || async move {
                hang.notify_one();
                std::future::pending::<&str>().await
            }),
        )
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
    let ended = tokio::time::timeout(Duration::from_secs(10), served)
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
    let ended = tokio::time::timeout(Duration::from_secs(10), served)
        .await
        .expect("shutdown waited for its deadline with nothing open");

    assert!(ended.unwrap().is_ok());
    assert!(stopped_at.elapsed() < Duration::from_secs(10));
}
