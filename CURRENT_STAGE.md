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

After moving the shared authority test helpers into `daemon/test_support.rs`,
the package still passed **202 passed, 0 failed, 3 ignored**. After adding the
connection worker, package tests passed with **212 passed, 0 failed, 3 ignored**
(ten new worker tests, repeated 30 times without a failure); `cargo check`,
formatting verification, and all-target Clippy also passed without warnings.
The suite passed inside the sandbox, and the ignored tests were not run. The
worker tests use `UnixStream::pair()` and `FakeBackend`; they do not bind a
listener or need a real Hyprland session.

After adding the coordinator and the authority exit hook, package tests passed
with **225 passed, 0 failed, 3 ignored** (three new authority hook tests and ten
new coordinator tests; the thirteen new tests were repeated 30 times, and the
whole `daemon::` suite 30 times, without a failure); `cargo check`, formatting
verification, and all-target Clippy also passed without warnings. The suite
passed inside the sandbox, and the ignored tests were not run. The coordinator
tests use the real listener in a private temporary runtime root through a
test-only `UnixRuntimeListener::bind_at`, `FakeBackend`, and in-test backends;
they do not need a real Hyprland session.

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
  protocol response. The `B: WindowBackend + Send + 'static` bound applies only
  at the concurrent boundaries (`Authority::spawn`,
  `Authority::spawn_with_exit_hook`, and `Coordinator::start`), not to the
  `WindowBackend` trait; a compile-time test checks that `HyprlandBackend`
  satisfies it. An authority panic is fatal and is not
  replaced; `Authority::shutdown` reports it as `Panicked`.
  `Authority::spawn_with_exit_hook` also runs a hook once when the authority
  thread ends, including on panic. The hook must not panic, and the drop guard
  contains a panicking hook so it cannot abort the process. `Authority::spawn`
  keeps its signature and behavior and delegates to the same code without a hook.
- Connection worker (`daemon::spawn_worker`): one thread named
  `clea-windowd-worker` that serves one accepted `UnixStream` through
  `serve_unix_connection_with_handler`, forwarding each request line to an
  `AuthorityClient`. It implements no framing and never sees the service or the
  backend. The thread returns a `WorkerExit` (`Completed`, `Transport`,
  `Preparation`, or `Authority`), and none of the failure variants becomes a
  protocol response: the client observes only EOF. `WorkerHandle` offers
  `is_finished`, `join`, and `close_connection`, which shuts down the socket
  through a duplicated descriptor so a worker blocked on an idle client ends.
  A `CloseOnDrop` guard shuts the socket down when the worker ends, including
  on panic, so the client sees EOF even while the coordinator's duplicate is
  alive. A panicking worker closes only its own connection and does not affect
  the authority or other workers. `WorkerLifecycleError` reports
  `ControlHandleFailed` or `SpawnFailed` from `spawn_worker` and `Panicked` from
  `join`.
- Daemon coordinator (`daemon::Coordinator`, [ADR 0007](docs/architecture/adr/0007-window-management-daemon-coordinator.md),
  status Proposed): `Coordinator::start` takes the bound `UnixRuntimeListener`
  and the built service, starts the authority, and runs an accept thread named
  `clea-windowd-accept` that owns the listener, the authority, and the worker
  handles. It starts one worker per accepted stream, reaps finished workers on
  each accept, and closes a connection whose worker cannot be created. A
  cloneable `ShutdownHandle` queues a lifecycle event and wakes the blocked
  `accept` by connecting to the daemon's own service socket; the connection is
  made only if the event was delivered, and the event receiver is dropped as soon
  as the loop ends. The authority's exit hook reports a failure the same way, so
  an authority failure is detected without a new client. The shutdown sequence
  is: leave the loop, close every worker connection, join the workers, shut the
  authority down, and clean up the listener by device and inode. `wait` returns
  a `CoordinatorReport` with the stop reason (`ShutdownRequested`,
  `AuthorityFailed`, or `AcceptFailed`), worker counts, and the result of each
  shutdown step. `Coordinator::start` is the only coordinator item requiring
  `B: WindowBackend + Send + 'static`.

## Not implemented

- Signal handling: nothing calls `ShutdownHandle::request_shutdown` from a
  signal yet.
- Functional executable entry point (`main.rs` is empty).
- Automatic reaction to compositor events and reconciliation.
- Persistence of modes or external identities.
- Shell/Settings integration; those directories currently contain only READMEs.
- Packaging metadata and session integration.

Services and configuration also remain documentation-only component boundaries.
The library components do not yet form an operational desktop daemon.

## Next milestone

Plan and implement signal handling and a functional `main.rs` on top of the
coordinator: start the real `HyprlandBackend`, bind the listener, call
`Coordinator::start`, connect signals to `ShutdownHandle::request_shutdown`, and
end the process when the coordinator stops or panics. The plan must be
presented and authorized before implementation.

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
  dropped. The coordinator closes every worker connection with
  `WorkerHandle::close_connection`, which does not wake a worker that is waiting
  for the authority inside a request, so shutdown waits for a slow backend
  operation.
- A panic of the coordinator thread is fatal for the process:
  `CoordinatorError::Panicked` leaves the workers and the authority running,
  detached, until the process exits. The future `main.rs` must observe
  `Coordinator::is_finished` or call `Coordinator::wait` and end the process.
- Accept errors are fatal and are not classified, because
  `UnixRuntimeError::AcceptFailed` does not keep the `io::ErrorKind`; telling
  transient from fatal errors requires preserving it in the Unix runtime. If the
  socket file is removed or replaced, the wake-up connection fails
  (`ShutdownError::WakeFailed`) and the accept loop stays blocked until a client
  connects. The wake-up connect can also block in a rare race: the receiver is
  dropped between a successful send and the connect, and the listener backlog is
  full.
- The coordinator and workers have no limits yet: no maximum number of
  connections (each costs a thread and three to four descriptors: the stream,
  the control duplicate, the cleanup guard, and the runtime's reader clone), no
  maximum line size (the framing reads an unbounded line), no timeout for idle
  connections, and no logging, so `WorkerExit` and `CoordinatorReport` are the
  only diagnostics. Finished workers are reaped only when a connection is
  accepted or at shutdown, and each holds a descriptor until then. Under
  descriptor exhaustion `try_clone` fails and the connection is refused. The
  extra duplicates exist because `serve_unix_connection_with_handler` takes the
  stream by value.
- Not covered by tests, because they cannot be forced deterministically:
  `ControlHandleFailed` and `SpawnFailed`, accept errors, a failure to create the
  coordinator thread, a panic of the coordinator thread, and the race in the
  wake-up connect described above.
- Production signal handling, supervision, persistence, and compatibility with
  other Hyprland versions remain future work.

Architecture and contracts live in [docs/architecture](docs/architecture/).
