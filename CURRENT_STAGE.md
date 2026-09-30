# Current Stage

## Repository and validated baseline

- Branch at validation: `feature/window-management-core`.
- Rust package: `clea-windowd`, in `window-management/Cargo.toml`; there is no
  separate root Cargo manifest or multi-package workspace.
- Implementation baseline: commit `974f27de0a45a6f217826ac99314c0198bfb7861`.
- Validation date: 2026-09-29, using Rust and Cargo **1.98.1**.
- `cargo check`, formatting verification, and Clippy for all targets passed;
  Clippy emitted no warnings.
- Tests: **172 passed, 0 failed, 3 ignored**. The ignored tests require a real
  Hyprland session; two mutate live windows or a workspace and were not run.
- The initial sandbox run had 35 failures because local Unix socket creation
  was blocked. The authorized run outside the sandbox passed. Reproduction
  requires permission to create local Unix sockets, but not a real compositor
  for the non-ignored suite.
- The Rust toolchain is not pinned in the repository. `Cargo.lock` records
  dependency resolution; it does not pin Rust or Cargo.

After the parser precedence fix, package tests passed with **175 passed,
0 failed, 3 ignored**; formatting verification and all-target Clippy also passed
without warnings. The implementation baseline above records the earlier run.

After the handler-based transport refactor, package tests
passed with **183 passed, 0 failed, 3 ignored** (eight new transport tests);
`cargo check`, formatting verification, and all-target Clippy also passed
without warnings. In this run the suite passed inside the sandbox, so the
earlier socket-creation failures did not recur; they remain a possible
environment-dependent outcome. The ignored tests were not run.

After adding the handler variant of the Unix connection helper, package tests
passed with **188 passed, 0 failed, 3 ignored** (five new Unix runtime tests);
`cargo check`, formatting verification, and all-target Clippy also passed
without warnings. The suite again passed inside the sandbox, and the ignored
tests were not run.

After adding the service authority (`daemon` module), package tests passed with
**202 passed, 0 failed, 3 ignored** (fourteen new authority tests, repeated 30
times without a failure); `cargo check`, formatting verification, and all-target
Clippy also passed without warnings. The suite passed inside the sandbox, and
the ignored tests were not run. The new tests use `FakeBackend`, in-test
backends, and `UnixStream::pair()`, not a real Hyprland session.

Validated commands, run from the repository root:

```sh
cargo check --manifest-path window-management/Cargo.toml --workspace --all-targets --locked
cargo test --manifest-path window-management/Cargo.toml --workspace --locked
cargo fmt --manifest-path window-management/Cargo.toml --all -- --check
cargo clippy --manifest-path window-management/Cargo.toml --workspace --all-targets --locked
```

## Implemented

- `WindowManager`: in-memory default and explicit workspace modes, effective
  mode resolution, and explicit placement application through the core planner.
- Protocol v1: typed validation, nine client methods, responses, error mapping,
  and opaque external identity tokens through `WindowManagementService`.
- JSON Lines transport: sequential requests, LF framing, continued processing
  after protocol errors, and separate transport failures. The single framing
  implementation is `serve_connection_with_handler`, which calls a handler once
  per complete line and returns `ServeError<E>` (`Transport` or `Handler`);
  `serve_connection` is a wrapper with an unchanged signature. The Unix runtime
  adapter exposes the same handler model through
  `serve_unix_connection_with_handler`; no coordinator uses it yet.
- `FakeBackend`: offline observations, declarative actions, action recording,
  and failure simulation.
- `HyprlandBackend`: native synchronous IPC reads, snapshot normalization,
  declarative placement and focus actions, and postcondition verification.
  The documented compatibility target is Hyprland **0.56.2** only.
- Unix runtime: socket binding, permissions, active-listener detection,
  narrowly scoped stale-socket recovery, one-connection adaptation, and
  identity-checked cleanup. `serve_unix_connection_with_handler` serves one
  accepted stream through a request handler and returns
  `UnixServeError<E>` (`Runtime`, `Transport`, or `Handler`), which keeps the
  handler's error out of `UnixRuntimeError`. A handler error closes the
  connection without a response, so the client observes only EOF.
  `serve_unix_connection` keeps its signature and behavior.
- Service authority (`daemon::Authority`): one thread that exclusively owns the
  `WindowManagementService`, admitting `ServiceCall` values one at a time
  through a zero-capacity `std::sync::mpsc` rendezvous channel, with a
  one-response channel per call. `AuthorityClient::submit` is the worker-facing
  handler; it returns `AuthorityError` (`Unavailable` when the call was not
  admitted, `ResponseLost` when it was admitted but no response arrived), which
  fits the transport and Unix runtime `Handler(E)` variants and never becomes a
  protocol response. `Authority::spawn` is the only place requiring
  `B: WindowBackend + Send + 'static`; a compile-time test checks that
  `HyprlandBackend` satisfies it. An authority panic is fatal and is not
  replaced; `Authority::shutdown` reports it as `Panicked`.

## Not implemented

- Multi-client daemon coordinator: accept/lifecycle role, one connection worker
  per client, worker spawn-failure handling, fatal-authority handling by the
  accept role, and listener cleanup. The authority exists, but nothing spawns it
  outside tests.
- Functional executable entry point (`main.rs` is empty).
- Automatic reaction to compositor events and reconciliation.
- Persistence of modes or external identities.
- Shell/Settings integration; those directories currently contain only READMEs.
- Packaging metadata and session integration.

Services and configuration also remain documentation-only component boundaries.
The library components do not yet form an operational desktop daemon.

## Next milestone

Implement and test the **daemon execution coordinator** from
[ADR 0006](docs/architecture/adr/0006-window-management-daemon-execution.md)
independently of `main.rs`, on top of the existing authority: an accept/lifecycle
role that owns the `UnixRuntimeListener`, one connection worker per client using
the handler-based serving functions, and reused JSON Lines framing. Verify
worker spawn failure, fatal-authority handling that stops new admissions, and
controlled shutdown before wiring the executable.

## Known risks and limitations

- Snapshot normalization requires a correlatable active window; the no-active
  window case remains unresolved.
- Hyprland reads and actions are synchronous, without timeouts or retries.
  A blocked backend operation could stall the future serial authority.
- Snapshots and actions are non-atomic. Workspace/fullscreen changes between
  observation and action are races; continuous invariants under those races
  have not been demonstrated.
- Placement application stops on the first failure without rollback. Policy
  setters do not apply placement, and partial application can leave logical
  mode and observed placement different.
- Identity maps retain historical entries; line-size and future connection/
  worker limits are not specified.
- Controlled shutdown depends on the connection workers ending:
  `Authority::shutdown` returns only after every `AuthorityClient` clone is
  dropped, and a worker blocked on an idle client holds one. The coordinator will
  need to close the connection streams to end its workers.
- Production signal handling, supervision, persistence, and compatibility with
  other Hyprland versions remain future work.

Architecture and contracts live in [docs/architecture](docs/architecture/).
