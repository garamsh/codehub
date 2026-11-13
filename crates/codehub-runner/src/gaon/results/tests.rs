//! Tests of the result listener, over real HTTP.

use super::*;
use crate::run_job::PostedEnd;

async fn listen(results: &Results) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = results.router();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
}

fn posted() -> Posted {
    Posted {
        end: PostedEnd::Exited(3),
        output: "tail".to_owned(),
    }
}

async fn post(base: &str, job: u64, bearer: Option<&str>, body: &str) -> StatusCode {
    let mut request = reqwest::Client::new()
        .post(format!("{base}/results/{job}"))
        .header("content-type", "application/json")
        .body(body.to_owned());
    if let Some(bearer) = bearer {
        request = request.bearer_auth(bearer);
    }
    request.send().await.unwrap().status()
}

fn body() -> String {
    serde_json::to_string(&posted()).unwrap()
}

#[tokio::test]
async fn a_post_with_the_job_secret_is_taken_and_a_second_is_refused() {
    let results = Results::default();
    let base = listen(&results).await;
    let mut expectation = results.expect(1, "right");
    assert_eq!(post(&base, 1, Some("right"), &body()).await, StatusCode::OK);
    assert_eq!(expectation.take(), Some(posted()));
    assert_eq!(
        post(&base, 1, Some("right"), &body()).await,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn a_post_with_another_secret_is_refused_and_the_job_still_takes_its_own() {
    let results = Results::default();
    let base = listen(&results).await;
    let mut expectation = results.expect(1, "right");
    for bearer in [Some("wrong"), Some(""), None] {
        assert_eq!(
            post(&base, 1, bearer, &body()).await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(expectation.take(), None, "nothing was accepted");
    assert_eq!(post(&base, 1, Some("right"), &body()).await, StatusCode::OK);
    assert_eq!(expectation.take(), Some(posted()));
}

#[tokio::test]
async fn the_secret_of_one_job_does_not_post_for_another() {
    let results = Results::default();
    let base = listen(&results).await;
    let mut one = results.expect(1, "one");
    let mut two = results.expect(2, "two");
    assert_eq!(
        post(&base, 2, Some("one"), &body()).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(&base, 3, Some("one"), &body()).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!((one.take(), two.take()), (None, None));
}

#[tokio::test]
async fn a_post_for_a_job_no_longer_awaited_is_refused() {
    let results = Results::default();
    let base = listen(&results).await;
    drop(results.expect(1, "right"));
    assert_eq!(
        post(&base, 1, Some("right"), &body()).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_malformed_body_is_refused_without_using_up_the_job_post() {
    let results = Results::default();
    let base = listen(&results).await;
    let mut expectation = results.expect(1, "right");
    assert_eq!(
        post(&base, 1, Some("right"), "{").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(post(&base, 1, Some("right"), &body()).await, StatusCode::OK);
    assert_eq!(expectation.take(), Some(posted()));
}

#[test]
fn digests_are_the_same_only_where_every_byte_is() {
    let a = [7_u8; 32];
    assert!(same(&a, &a));
    for at in [0, 31] {
        let mut b = a;
        b[at] ^= 1;
        assert!(!same(&a, &b), "differs at {at}");
    }
}
