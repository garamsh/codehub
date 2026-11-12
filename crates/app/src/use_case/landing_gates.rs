//! The gates that let the landings of one repository pass one at a time, in arrival order.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

use crate::domain::repository::RepositoryId;

/// One gate per repository that a landing is passing or waiting at. A repository's gate lasts only
/// while a landing holds or waits at it. Landings pass a gate in the order they reached it: a
/// gate that a landing leaves is handed to the landing that has waited longest, so a landing that
/// arrives later never takes it first.
#[derive(Debug, Default)]
pub struct LandingGates {
    gates: Mutex<HashMap<RepositoryId, Arc<Gate>>>,
}

impl LandingGates {
    /// Gates with no landing at any of them.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reaches the gate of `repository`. The landing passes by waiting on the returned [`Arrival`].
    #[must_use]
    pub fn reach(&self, repository: RepositoryId) -> Arrival<'_> {
        let gate = lock(&self.gates).entry(repository).or_default().clone();
        Arrival {
            gates: self,
            repository,
            gate,
        }
    }

    /// How many repositories have a landing at their gate.
    #[must_use]
    pub fn open(&self) -> usize {
        lock(&self.gates).len()
    }
}

/// A landing that reached the gate of a repository. Dropping it, passed or not, leaves the gate.
#[derive(Debug)]
pub struct Arrival<'a> {
    gates: &'a LandingGates,
    repository: RepositoryId,
    gate: Arc<Gate>,
}

impl Arrival<'_> {
    /// Waits for the gate and holds it until the returned [`Pass`] is dropped.
    pub async fn pass(&self) -> Pass<'_> {
        Passing {
            gate: &self.gate,
            ticket: None,
        }
        .await
    }
}

impl Drop for Arrival<'_> {
    fn drop(&mut self) {
        let mut gates = lock(&self.gates.gates);
        // The map and this arrival hold the two references where no other landing is at the gate.
        if Arc::strong_count(&self.gate) == 2 {
            gates.remove(&self.repository);
        }
    }
}

/// The hold on a gate. Dropping it hands the gate to the next landing waiting.
#[derive(Debug)]
pub struct Pass<'a> {
    gate: &'a Gate,
}

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        self.gate.hand_over();
    }
}

#[derive(Debug, Default)]
struct Gate {
    state: Mutex<Queue>,
}

#[derive(Debug, Default)]
struct Queue {
    held: bool,
    next_ticket: u64,
    /// The waiting landings, longest first, each with the waker of its last poll.
    waiting: VecDeque<(u64, Waker)>,
    /// The ticket the gate was handed to, until that landing takes it.
    handed_to: Option<u64>,
}

impl Gate {
    /// Gives the held gate to the landing that has waited longest, or frees it.
    fn hand_over(&self) {
        let mut queue = lock(&self.state);
        match queue.waiting.pop_front() {
            Some((ticket, waker)) => {
                queue.handed_to = Some(ticket);
                drop(queue);
                waker.wake();
            }
            None => queue.held = false,
        }
    }
}

/// A landing waiting its turn at a gate.
struct Passing<'a> {
    gate: &'a Gate,
    ticket: Option<u64>,
}

impl<'a> Future for Passing<'a> {
    type Output = Pass<'a>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Pass<'a>> {
        let gate = self.gate;
        let mut queue = lock(&gate.state);
        match self.ticket {
            None if !queue.held => {
                queue.held = true;
                Poll::Ready(Pass { gate })
            }
            None => {
                let ticket = queue.next_ticket;
                queue.next_ticket = ticket.wrapping_add(1);
                queue.waiting.push_back((ticket, context.waker().clone()));
                drop(queue);
                self.ticket = Some(ticket);
                Poll::Pending
            }
            Some(ticket) if queue.handed_to == Some(ticket) => {
                queue.handed_to = None;
                drop(queue);
                self.ticket = None;
                Poll::Ready(Pass { gate })
            }
            Some(ticket) => {
                if let Some((_, waker)) = queue.waiting.iter_mut().find(|(t, _)| *t == ticket) {
                    waker.clone_from(context.waker());
                }
                Poll::Pending
            }
        }
    }
}

impl Drop for Passing<'_> {
    fn drop(&mut self) {
        let Some(ticket) = self.ticket else { return };
        let mut queue = lock(&self.gate.state);
        if queue.handed_to == Some(ticket) {
            queue.handed_to = None;
            drop(queue);
            self.gate.hand_over();
        } else {
            queue.waiting.retain(|(t, _)| *t != ticket);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
