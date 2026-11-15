# ADR 0050: Ports are held as shared trait objects, through async-trait; the domain ring stays synchronous

> Status: accepted
> Date: 2026-11-16
> Supersedes: ADR 0018 (its dispatch; the synchronous domain ring and the place of the runtime stand)

Append-only: once merged, the body below is not rewritten. A fact later found wrong is corrected in an appended note, not edited out; a revised decision is a new ADR that supersedes this one.

## Context

This ADR comes from goal #212, decision 11, and issue #219. On 2026-11-16 the owner granted the PM the right to supersede ADR 0018 if its own "Signs the decision was wrong" hold. The superseding form needs a change to `docs/convention/stack-rust.md`, which that grant did not cover; the owner left that choice to the PM on the same day (#212, issuecomment-6898841817). The architecture scan of 2026-11-15 found that they do. Checked at main 6519e40; the cited lines are unchanged at 280e5d5:

- **First sign: use-case and adapter types carry long lists of port type parameters.** It holds:
  - `Assembly` takes 15 type parameters (`crates/app/src/use_case/assembly.rs:18`), `JobsService` 9, and `ChecksService` and `ShippedSourceControl` 8 each.
  - The harnesses keep towers of type aliases to name them: `crates/test-support/src/stores.rs:45-61` and `crates/test-support/src/contract/runner_deployment.rs:27-83`.
  - A test that substitutes one port must spell out every other one. #208's counting wrapper had to build `JobsService` by hand, because `Assembly` cannot swap one type.
- **Second sign: a deployment selects an implementation from configuration, and enums stand in for `dyn`.** It holds:
  - The composition site keeps three enums, `Changes`, `Verifier` and `Decider` (`crates/codehub/src/lib.rs:164-254`), each forwarding every port method by hand.
  - The runner writes its claim loop once per executor variant (`crates/codehub-runner/src/lib.rs:120-155`), because each executor is a different type.
  - ADR 0018 rested on the premise that "the composition site constructs exactly one implementation for each port, so the concrete type is known where the graph is built". Both places now choose the implementation at run time, so that premise no longer holds there.
- The other three signs do not hold. No adapter starts its own runtime, no implementation needs a non-`Send` resource across an `.await`, and the domain is not split into resumable steps. The parts of ADR 0018 those signs guard stand.

Facts the decision rests on:
- In rustc 1.99.1 a trait with an `async fn` or a return-position `impl Trait` method is still not dyn compatible (ADR 0018; The Rust Reference, Traits, Dyn compatibility).
- The Rust project's 2026 goal "Box notation for dyn async trait" (rust-lang.github.io/rust-project-goals/2026/afidt-box.html) is to settle a design. It is not a stabilisation, so native dynamic dispatch of async traits is not available on the toolchain this project pins.
- The `async-trait` crate (dtolnay, 0.1.89) rewrites each method to return `Pin<Box<dyn Future + Send + 'async_trait>>`, which makes the trait dyn compatible. It is the established workaround and what the ecosystem uses for it, and it costs one heap allocation per call.
- Every port this system has waits on I/O: the network, PostgreSQL, the object store, a process. One allocation per call is small beside a network round trip.

Alternatives not taken:
- **Keep static dispatch, and bundle the ports in one trait with associated types.** One `A: Adapters` parameter would replace the long lists, and that answers the first sign. The second sign would remain: the enums stay, and grow with every port chosen from configuration. Each test that substitutes one port would also need a bundle type of its own, which brings the alias towers back in another form.
- **The `dynosaur` crate,** which keeps static dispatch and boxes only behind `dyn`. It answers both signs with less allocation, but at 0.3 it has no 1.0 yet, and two forms of every port (static and `Dyn…`) are more to learn than one.
- **A hand-written `BoxFuture` return in each port method,** with no macro. It is the same mechanism as `async-trait`, with `Box::pin(async move { … })` written by hand in every one of some 55 implementations.
- **Wait for native dyn async traits.** That would leave both signs standing for an unknown time, while every new port chosen from configuration adds another enum.

## Decision

- **A port is a trait declared with `#[async_trait::async_trait]`,** with `Send + Sync` as supertraits. Its operations are written as `async fn`, and their futures are `Send`, as `async-trait` makes them by default.
- **A caller holds a port as `Arc<dyn Port>`,** or, for a single call, as `&dyn Port`. It is never a generic type parameter.
- **The composition site builds each implementation once and hands out clones of the `Arc`.** Choosing an implementation from configuration means building a different `Arc<dyn Port>`; no enum stands in for it.
- **This covers driven ports, driving ports and the runner's ports** (`JobSource`, `Executor`). Internal seams that are not ports (for example `RelayEnvironment`) may stay generic. Where one has two implementations chosen at run time, it takes the same form.
- **Unchanged from ADR 0018:**
  - the domain ring is synchronous;
  - `tokio` is a dependency only of tests, binaries and `crates/test-support`;
  - a port's futures are `Send`.
- The Rust form is in `docs/convention/stack-rust.md`.

## Consequences

- Use cases, services, `Assembly` and the harness deployments lose their port type parameters and alias towers. A test that substitutes one port passes a different `Arc`.
- The three composition-site enums and the runner's per-variant loop go.
- Every port call allocates one boxed future.
- `async-trait` becomes a dependency of `app`, of each adapter crate and of `codehub-runner`.
- A port method's error type, and the rule that a port's failure is an enum declared beside it, do not change.
- When native dyn async traits are stable on the pinned toolchain, moving off `async-trait` is mechanical: remove the attribute and adopt the native form. A new ADR records that move.

## Signs the decision was wrong

- Measured allocation or dispatch overhead shows up in the scale baseline (`docs/guide/scale-baseline.md`) beside the I/O it waits on.
- `async-trait`'s lifetimes or `Send` bounds force wrappers or `'static` copies that the generic form did not need.
- Native dyn-compatible async traits stabilise and the project has not moved off `async-trait` within a release of it.
