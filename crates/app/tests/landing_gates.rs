//! The gates that let the landings of one repository pass one at a time.

use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use app::domain::repository::RepositoryId;
use app::use_case::landing_gates::LandingGates;
use tokio::task::{JoinSet, yield_now};

fn repository(n: u64) -> RepositoryId {
    RepositoryId::new(n)
}

/// Lets every task that can run, run until it waits.
async fn settle() {
    for _ in 0..8 {
        yield_now().await;
    }
}

/// Starts a landing numbered `n` at the gate of repository 1 and lets it reach the gate. It
/// records that it passed, and holds the gate across a few turns, as a landing awaiting its ports.
async fn arrive(
    tasks: &mut JoinSet<()>,
    gates: &Arc<LandingGates>,
    passed: &Arc<Mutex<Vec<u32>>>,
    n: u32,
) {
    let (gates, passed) = (gates.clone(), passed.clone());
    tasks.spawn(async move {
        let arrival = gates.reach(repository(1));
        let _held = arrival.pass().await;
        passed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(n);
        settle().await;
    });
    yield_now().await;
}

#[tokio::test]
async fn landings_pass_in_the_order_they_arrived_while_new_ones_keep_arriving() {
    let gates = Arc::new(LandingGates::new());
    let passed = Arc::new(Mutex::new(Vec::new()));
    let mut tasks = JoinSet::new();
    let total = 40;
    let first = gates.reach(repository(1));
    let held = first.pass().await;
    for n in 0..total / 2 {
        arrive(&mut tasks, &gates, &passed, n).await;
    }
    drop(held);
    for n in total / 2..total {
        arrive(&mut tasks, &gates, &passed, n).await;
    }
    while tasks.join_next().await.is_some() {}

    let order = passed.lock().unwrap().clone();
    assert_eq!(order, (0..total).collect::<Vec<_>>());
}

#[tokio::test]
async fn a_repository_has_a_gate_only_while_a_landing_is_at_it() {
    let gates = LandingGates::new();
    assert_eq!(gates.open(), 0);

    let one = gates.reach(repository(1));
    let other = gates.reach(repository(2));
    assert_eq!(gates.open(), 2);
    drop(one.pass().await);
    drop(one);
    assert_eq!(gates.open(), 1);

    // A landing that reached the gate and left without passing leaves it too.
    let held = other.pass().await;
    let left = gates.reach(repository(2));
    drop(left);
    assert_eq!(gates.open(), 1);
    drop(held);
    drop(other);
    assert_eq!(gates.open(), 0);
}

#[tokio::test]
async fn a_landing_dropped_after_the_gate_was_handed_to_it_passes_the_gate_on() {
    let gates = Arc::new(LandingGates::new());
    let first = gates.reach(repository(1));
    let held = first.pass().await;
    let waiter = tokio::spawn({
        let gates = gates.clone();
        async move {
            let arrival = gates.reach(repository(1));
            let _held = arrival.pass().await;
        }
    });
    settle().await;

    drop(held);
    waiter.abort();
    let _ = waiter.await;

    let next = gates.reach(repository(1));
    let mut pass = std::pin::pin!(next.pass());
    let polled = pass
        .as_mut()
        .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()));
    assert!(polled.is_ready());
}
