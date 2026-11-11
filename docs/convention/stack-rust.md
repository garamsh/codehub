# Rust — Workspace & Style Conventions

> How a Rust workspace is laid out and where each kind of code belongs:
> toolchain, crates, naming, errors, comments, tests, commands.
>
> Checked against rustc and cargo 1.99.1, clippy 0.1.99 and rustfmt
> 1.10.0, edition 2024. A claim below that names no version holds for
> these.

## Toolchain and workspace

- `rust-toolchain.toml` at the repository root pins the toolchain:
  `[toolchain]` with `channel` an exact release (`"1.99.1"`),
  `profile = "minimal"` and `components = ["rustfmt", "clippy"]`.
  `channel` accepts `stable`, `beta`, `nightly`, a release number or a
  dated nightly; only a release number or a dated one stays put.
- The root `Cargo.toml` is a virtual manifest: `[workspace]` with
  `members = ["crates/*"]` and `resolver = "3"` (a virtual workspace
  has no `package.edition` to infer it from, so cargo requires it).
- Shared settings live once in the root: `[workspace.package]` holds
  `edition = "2024"` and `rust-version`, `[workspace.dependencies]`
  holds every third-party dependency with its version, and
  `[workspace.lints]` holds the lint levels below. A member inherits
  them with `edition.workspace = true`, `rust-version.workspace =
  true`, `dep.workspace = true` and `[lints] workspace = true`.
- `Cargo.lock` is committed, and every entry point passes `--locked`,
  which fails where the lockfile would change instead of rewriting it.

## Crate layout

The workspace holds three kinds of package, all under `crates/`. Its
hexagonal unit takes these names:

- `crates/app` — the application crate. Its domain ring is
  `src/domain/` and the modules under it. Its application ring is the
  rest of the crate: the use cases, under `src/use_case/`, and the
  ports. `src/port/driving/<conversation>.rs` and
  `src/port/driven/<conversation>.rs` each declare one trait, named for
  the conversation; a directory exists once it holds a port. The port's
  error type is declared in the same file.
- `crates/adapter-<technology>` — one crate per technology, named for
  it (`adapter-postgres`, `adapter-http`). One module per port it
  implements, `src/<conversation>.rs`. `crates/adapter-memory` holds
  the in-process implementations of the driven ports that tests
  substitute.
- `crates/<binary>` — one package per shipped binary, named as the
  binary is installed (`ch`). It is both a library and a binary:
  `src/lib.rs` holds the composition site, the function that builds the
  adapters and passes them to the application, and `src/main.rs` only
  calls it. A binary-only package cannot be reached from `tests/`, so a
  test that wires the shipped graph calls that function.
- Direction is the dependency list. `app`'s `Cargo.toml` names no
  adapter; an adapter's names `app` and no other adapter; a binary
  package's names `app` and the adapters it wires, each from the change
  whose composition site first references it, so a binary package
  whose composition site references nothing from `app` does not name
  it. Cargo rejects a cycle between packages (`cyclic package
  dependency`), so an `app` that listed an adapter which lists `app`
  does not resolve.

## Async ports and the runtime

This section states where `async` appears, the form a port trait
takes, how a caller holds a port, and which packages name the runtime.

- **Nothing under `src/domain/` is `async`**, awaits, or names
  `Future`. Use cases, ports and adapters are where `async` appears.
- **A port operation is declared as a `fn` returning
  `impl Future<Output = …> + Send`**, and each implementation writes
  it as an `async fn`. The `Send` bound is what lets a caller spawn the
  future on a multi-threaded runtime (`tokio::spawn` requires
  `F: Future + Send + 'static`, tokio 1.54.1); an implementation whose
  future holds a non-`Send` value across `.await` does not compile. A
  bare `async fn` in a `pub` trait raises rustc's `async_fn_in_trait`
  warning, which fails the lint entry point, and its future cannot be
  passed where `Send` is required. An implementation that writes the
  declared form with an `async` block body raises
  `clippy::manual_async_fn`.
- **A caller holds a port as a generic type parameter**
  (`struct Land<S: ChangesetStore>`, `fn land<S: ChangesetStore>(…)`),
  and the composition site names the concrete type. A trait with such
  a method is not dyn compatible (The Rust Reference, Traits, Dyn
  compatibility: a dispatchable method must "Not be an `async fn`" and
  must not "have a return position `impl Trait` type"), so `dyn` of a
  port fails with `E0038`.
- **No `async-trait` crate.** `async fn` and return-position
  `impl Trait` in traits are native since Rust 1.75.0.
- **`tokio` is a `[dependencies]` entry only of a binary package**, and
  any package may name it under `[dev-dependencies]`. A test that
  awaits is `#[tokio::test]` (features `macros` and `rt`), which runs
  on a current-thread runtime; the `Send` bound is checked when the
  implementation compiles, not by the runtime.

## Naming

This section states the form each kind of Rust identifier takes.

- **Crates and packages:** kebab-case package names (`adapter-http`);
  the crate is `adapter_http` in code. No `-rs` or `-rust` affix.
- **Modules and files:** `snake_case`.
- **Types, traits, enum variants:** `UpperCamelCase`.
- **Functions, methods, locals:** `snake_case`. A getter takes the
  field's name without `get_` (`name()`); `as_` is a free
  borrowed-to-borrowed conversion, `to_` an expensive one, `into_` one
  that takes ownership.
- **Constants and statics:** `SCREAMING_SNAKE_CASE`.
- **Acronyms:** one word in camel case (`HttpClient`, `Uuid`), lowercase
  in snake case (`http_client`).

## Errors, types and trust boundaries

This section states the Rust forms: which type carries a failure, which
lints hold the line, and where a boundary parses.

- **Expected failure is `Result<T, E>`, and `E` is an enum** declared
  beside the port it belongs to, one variant per failure a caller
  branches on, implementing `std::error::Error`. `Result<T, ()>` and
  `Box<dyn Error>` do not appear in a port's signature. An adapter
  converts its driver's error into that enum (`From` or `map_err`)
  inside the function that calls the driver; the driver's error type
  does not leave the adapter crate.
- **Propagate with `?`.** `unwrap`, `expect`, indexing that can go out
  of range, `panic!`, `todo!` and `unimplemented!` are denied by the
  lint levels below; a slice is read with `get`.
- **Lint levels** in the root `Cargo.toml`: `[workspace.lints.rust]`
  sets `unsafe_code = "forbid"` and `missing_docs = "warn"`;
  `[workspace.lints.clippy]` sets `unwrap_used`, `expect_used`,
  `panic`, `todo`, `unimplemented`, `indexing_slicing` and
  `wildcard_enum_match_arm` to `"deny"`. Test code is exempt through
  `clippy.toml` at the root: `allow-unwrap-in-tests`,
  `allow-expect-in-tests`, `allow-panic-in-tests` and
  `allow-indexing-slicing-in-tests`, each `true`. Clippy applies them
  "in test functions or `#[cfg(test)]`": a `#[test]` function, or an
  item under a `#[cfg(test)]` attribute. Compiling an integration-test
  file with `--test` does not count, so in
  `crates/<package>/tests/<name>.rs` each helper function, and each
  `mod common;` declaration, carries `#[cfg(test)]`; without it, an
  `unwrap` in the helper fails the lint entry point.
- **A suppression is `#[expect(lint, reason = "...")]`**, never
  `#[allow]`: an `expect` whose lint no longer fires raises
  `unfulfilled_lint_expectations`, so the suppression cannot outlive
  its cause.
- **Domain primitives are tuple-struct newtypes** (`struct UserId(Uuid);`)
  with a private field and a constructor that returns `Result`.
- **A boundary parses with `TryFrom<Raw> for Type` or `FromStr`**, and
  the adapter's function returns the parsed type. State is an `enum`
  with data in its variants; a `match` over it names every variant,
  which `wildcard_enum_match_arm` enforces.

## Comments and docs

This section states the Rust comment syntax.

- `//` for inline comments; `///` above an item and `//!` at the top of
  a crate or module file for doc comments. The first paragraph, up to
  the first blank line, is the summary that rustdoc reuses in search and
  module overviews.
- A public item's doc comment carries `# Errors` where it returns
  `Result`, `# Panics` where it can panic, and `# Safety` on an `unsafe
  fn`. `missing_docs` flags the public item with no doc comment at all,
  and `clippy::missing_errors_doc` and `clippy::missing_panics_doc` flag
  the absent sections; set both to `"warn"` under
  `[workspace.lints.clippy]`.
- A code block in a doc comment is compiled and run by `cargo test` as a
  doc test, so it is an example that stays true or it fails.

## Testing

This section states the Rust test placement, runner and test-name form.

**Runner:** `cargo test` with the built-in `#[test]` harness; assertions
are `assert!`, `assert_eq!` and `assert_ne!`. A test may return
`Result<(), E>` and use `?` instead of `unwrap`.

**Organization:**

- **Unit tests:** `#[cfg(test)] mod tests` at the bottom of the file
  they test, reaching private items with `use super::*`.
- **Integration tests:** `crates/<package>/tests/<name>.rs`. Each file is
  its own crate and sees only the package's public API. Shared helpers
  go in `tests/common/mod.rs`, so that they are not compiled as a test
  of their own. A package whose tests substitute a port wires
  `crates/adapter-memory` in as a dev-dependency.
- **E2E tests:** the `e2e` package at the repository root, with
  `e2e/tests/<name>.rs`, added to `members` beside `crates/*`. The
  workspace test entry point excludes it with `--exclude e2e`.
- **In-process client:** the package's own library function, called from
  `tests/`. For a binary that a test must launch, cargo builds the
  package's binaries for its integration tests and sets
  `CARGO_BIN_EXE_<name>` to each path.
- Test names are `snake_case` sentences that read like a spec
  (`rejects_expired_token`); `cargo test` prints them with their module
  path.
- `cargo test <filter>` passes the filter to every test binary and exits
  0 when it matches nothing.

## Verification commands

The workspace root is the working directory. Every cargo invocation
takes `--locked`.

| Task | Command |
|------|---------|
| Build | `cargo build --workspace --all-targets --locked` |
| Test | `cargo test --workspace --locked` (add `--exclude e2e` once `e2e` exists) |
| Test (one) | `cargo test -p <package> <filter> --locked` |
| Test (e2e) | `cargo test -p e2e --locked` |
| Lint | `cargo clippy --workspace --all-targets --locked`, run with `CARGO_BUILD_WARNINGS=deny` |
| Format check | `cargo fmt --all --check` |
| Format apply | `cargo fmt --all` |

`CARGO_BUILD_WARNINGS=deny` (the `build.warnings` setting, cargo 1.97 and
later) makes any warning fail the run, rustc's included, and unlike
`-D warnings` it does not invalidate the build cache. Set it on the lint
entry point alone.

`cargo test` prints `running N tests` per test binary and `test result`
per binary, and exits 0 where `N` is 0 everywhere: a package with no
tests, or a filter that matches none, passes. The test entry point sums
the `running N tests` lines it receives, prints the total beside the
selector `--workspace`, and fails where the total is zero. It passes
any arguments it is given through to `cargo test` and prints them
after `--workspace`, verbatim, in the selector (`3 tests ran
(--workspace version)`), so a filtered run never reads as the full
one. Over a workspace with no members cargo itself exits 101 (`The
manifest is virtual, and the workspace has no members`), so a member
count is not checked separately.

`--workspace` selects the packages `members` lists, and the glob
`crates/*` takes the directories on disk. `cargo test` without
`--all-targets` runs the doc tests of each library; `--all-targets`
does not, and `--doc` cannot be combined with it, so Test is the entry
point that runs them. `cargo fmt --all` reaches the files each target's
`mod` declarations reach, so a `.rs` file nothing declares is neither
checked nor rewritten.

A task runner wraps these under the entry-point names; the cargo
commands stay the same.
