//! A counting HTTP pass-through proxy: every request is forwarded to one upstream origin
//! unchanged, and counted.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::Failure;

/// The largest body the proxy forwards in either direction.
const BODY_LIMIT: usize = 1 << 20;

/// Headers that describe one connection and are not forwarded.
const CONNECTION_HEADERS: [&str; 7] = [
    "host",
    "connection",
    "keep-alive",
    "transfer-encoding",
    "content-length",
    "te",
    "upgrade",
];

struct Shared {
    http: reqwest::Client,
    upstream: String,
    count: AtomicU64,
}

/// A proxy to one upstream origin, listening on a port of its own until dropped.
pub struct Proxy {
    shared: Arc<Shared>,
    /// The origin to point a client at.
    pub origin: String,
    task: JoinHandle<()>,
}

impl Proxy {
    /// Starts a proxy to `upstream`, an origin such as `http://127.0.0.1:18082`, on a free port.
    ///
    /// # Errors
    ///
    /// Fails where no port can be bound.
    pub async fn start(upstream: &str) -> Result<Self, Failure> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let shared = Arc::new(Shared {
            http: reqwest::Client::new(),
            upstream: upstream.trim_end_matches('/').to_owned(),
            count: AtomicU64::new(0),
        });
        let app = Router::new().fallback(forward).with_state(shared.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            shared,
            origin,
            task,
        })
    }

    /// How many requests have arrived.
    pub fn count(&self) -> u64 {
        self.shared.count.load(Ordering::Relaxed)
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn copy_headers(from: &HeaderMap) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in from {
        if !CONNECTION_HEADERS.contains(&name.as_str()) {
            headers.append(name.clone(), value.clone());
        }
    }
    headers
}

fn bad_gateway() -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::BAD_GATEWAY;
    response
}

async fn forward(State(shared): State<Arc<Shared>>, request: Request) -> Response {
    shared.count.fetch_add(1, Ordering::Relaxed);
    let (parts, body) = request.into_parts();
    let Ok(body) = to_bytes(body, BODY_LIMIT).await else {
        return bad_gateway();
    };
    let target = match parts.uri.path_and_query() {
        Some(path) => format!("{}{path}", shared.upstream),
        None => shared.upstream.clone(),
    };
    let sent = shared
        .http
        .request(parts.method, target)
        .headers(copy_headers(&parts.headers))
        .body(body)
        .send()
        .await;
    let Ok(answer) = sent else {
        return bad_gateway();
    };
    let status = answer.status();
    let headers = copy_headers(answer.headers());
    let Ok(bytes) = answer.bytes().await else {
        return bad_gateway();
    };
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

#[cfg(test)]
mod tests {
    use axum::routing::post;

    use super::*;

    async fn upstream() -> String {
        let app = Router::new().route(
            "/echo",
            post(|body: String| async move { format!("got {body}") }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        origin
    }

    #[tokio::test]
    async fn forwards_each_request_unchanged_and_counts_it() {
        let proxy = Proxy::start(&upstream().await).await.unwrap();
        let client = reqwest::Client::new();

        for round in 1..=3 {
            let answer = client
                .post(format!("{}/echo", proxy.origin))
                .body("ping")
                .send()
                .await
                .unwrap();
            assert_eq!(answer.status(), 200);
            assert_eq!(answer.text().await.unwrap(), "got ping");
            assert_eq!(proxy.count(), round);
        }

        // The control: a path the upstream does not serve is answered by the upstream with its 404
        // and counted, so the proxy passes statuses through and is not what answered.
        let missing = client
            .post(format!("{}/other", proxy.origin))
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), 404);
        assert_eq!(proxy.count(), 4);
    }
}
