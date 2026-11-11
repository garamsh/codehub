# ADR 0018: Ports are asynchronous traits dispatched statically, and the domain ring is synchronous

> Status: accepted
> Date: 2026-11-11

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

The decision is decision 7 of issue #19, whose choices the user delegated to the PM on 2026-11-11. Milestone M1 writes the first ports, use cases and adapters (issue #19, decision 1), so the form a port takes in Rust is met now.

Every driven port this system has named waits on something outside the process: the object store, PostgreSQL (ADR 0013), and the HTTP server API (ADR 0015) on the driving side. The domain ring performs no I/O (ADR 0014), so it has nothing to wait on.

Facts the decision rests on:

- `async fn` and return-position `impl Trait` in traits are stable since Rust 1.75.0 (*Announcing `async fn` and return-position `impl Trait` in traits*, Rust Blog, 2023-12-21).
- A trait with such a method is not dyn compatible: a dispatchable method must "Not be an `async fn`" and must not "have a return position `impl Trait` type" (The Rust Reference, Traits, Dyn compatibility). rustc 1.99.1 rejects `&dyn` of such a trait with `E0038`.
- A bare `async fn` in a public trait raises the warn-by-default `async_fn_in_trait` lint in rustc 1.99.1, because its future's auto traits, `Send` among them, cannot be stated; the lint's help offers a `fn` returning `impl Future<Output = …> + Send` instead. A generic caller cannot pass a bare `async fn`'s future where `Send` is required (`E0277`).
- `tokio::spawn` requires `F: Future + Send + 'static` (tokio 1.54.1), so a future that is not `Send` cannot be spawned on its multi-threaded runtime.

Alternatives not taken:

- **Synchronous ports.** Each adapter that waits on I/O would block its thread or run a runtime of its own inside a synchronous call, and moving to asynchronous ports later changes every port signature and every caller.
- **The `async-trait` crate.** It rewrites each method to return a boxed `dyn Future`, one heap allocation per call and a procedural macro in every crate that declares or implements a port, for what the language has supported natively since 1.75.0. Its remaining advantage, dyn compatibility, is the next alternative.
- **Dynamic dispatch (`dyn Port`).** Unavailable to a native asynchronous trait without boxing, and not needed: the composition site constructs exactly one implementation for each port, so the concrete type is known where the graph is built.
- **A bare `async fn` in the trait.** It fails the lint entry point under `CARGO_BUILD_WARNINGS=deny` unless suppressed, and leaves a generic caller unable to spawn the future on a multi-threaded runtime.

## Decision

A port is a trait whose operations return futures, declared as a `fn` returning `impl Future<Output = …> + Send` and dispatched statically: a caller takes the port as a generic type parameter, never as `dyn`, and no `async-trait` crate is used. The domain ring is synchronous. The async runtime, `tokio`, is a dependency only of tests and binaries. The Rust form is in `docs/convention/stack-rust.md`.

## Consequences

- A use case awaits ports and calls domain functions; a domain function awaits nothing, so I/O stays sequenced in use cases as ADR 0014 requires.
- Every implementation of a port returns a `Send` future; one that holds a non-`Send` value across an `.await` does not compile, which a single-threaded implementation must work around.
- Each use case and driving adapter is generic over the ports it holds, and the composition site names the concrete types; choosing an implementation at run time needs an enum over the implementations, since `dyn` is unavailable.
- `app` and the adapter crates compile without a runtime; tests and binaries choose it.
- Changing a port's future to drop `Send` later is a breaking change to every caller that relies on it.

## Signs the decision was wrong

- Use-case and adapter types carry long lists of port type parameters, or the generic instantiations measurably dominate build time.
- A deployment needs to select an implementation from configuration, and enums wrapping implementations appear to stand in for `dyn`.
- A driver an adapter needs requires the runtime as a normal dependency of the adapter crate, or an adapter starts its own runtime.
- A port implementation needs a non-`Send` resource across an `.await`, and wrappers appear only to satisfy the bound.
- Domain code is split into resumable steps or callbacks so that a use case can await between them, which is the waiting the domain ring was meant not to do.
