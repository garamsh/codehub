//! The result listener: accepts the one result each running job posts, from the caller that holds
//! the job's secret.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::post;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use crate::host::OUTPUT_LIMIT;
use crate::run_job::Posted;

/// The most a posted result may weigh, in bytes: an output tail of control bytes, each of which
/// JSON writes in 6, and room for the rest of the body.
const BODY_LIMIT: usize = 6 * OUTPUT_LIMIT + 4 * 1024;

struct Slot {
    digest: [u8; 32],
    sender: Option<oneshot::Sender<Posted>>,
}

/// The jobs whose results are awaited.
#[derive(Clone, Default)]
pub struct Results(Arc<Mutex<HashMap<u64, Slot>>>);

/// The wait for one job's result. Dropping it stops the listener accepting that job's post.
pub struct Expectation {
    job: u64,
    results: Results,
    receiver: oneshot::Receiver<Posted>,
}

impl Expectation {
    /// The result, where it has arrived.
    pub fn take(&mut self) -> Option<Posted> {
        self.receiver.try_recv().ok()
    }
}

impl Drop for Expectation {
    fn drop(&mut self) {
        self.results.slots().remove(&self.job);
    }
}

fn digest(text: &[u8]) -> [u8; 32] {
    Sha256::digest(text).into()
}

/// Whether two digests are equal, in time that does not depend on where they differ.
fn same(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0_u8, |all, (x, y)| all | (x ^ y)) == 0
}

impl Results {
    fn slots(&self) -> MutexGuard<'_, HashMap<u64, Slot>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Awaits the one result of `job`, posted with `secret`.
    #[must_use]
    pub fn expect(&self, job: u64, secret: &str) -> Expectation {
        let (sender, receiver) = oneshot::channel();
        self.slots().insert(
            job,
            Slot {
                digest: digest(secret.as_bytes()),
                sender: Some(sender),
            },
        );
        Expectation {
            job,
            results: self.clone(),
            receiver,
        }
    }

    /// The listener's routes.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/results/{job}", post(accept))
            .layer(DefaultBodyLimit::max(BODY_LIMIT))
            .with_state(self.clone())
    }
}

async fn accept(
    State(results): State<Results>,
    Path(job): Path<u64>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let offered = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    let offered = digest(offered.as_bytes());
    let mut slots = results.slots();
    // An unknown job is compared against nothing, so the answer takes as long as any other.
    let known = slots.get_mut(&job);
    let expected = known.as_ref().map_or([0; 32], |slot| slot.digest);
    let authorised = same(&offered, &expected) && known.is_some();
    let Some(slot) = known.filter(|_| authorised) else {
        return StatusCode::UNAUTHORIZED;
    };
    let Ok(posted) = serde_json::from_slice::<Posted>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    match slot.sender.take() {
        Some(sender) => {
            // The waiting side may have given up; the post was still the job's one.
            let _ = sender.send(posted);
            StatusCode::OK
        }
        None => StatusCode::CONFLICT,
    }
}

#[cfg(test)]
mod tests;
