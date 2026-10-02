# Current Stage

## Rename from CLEA to Nycti

- The project was renamed from CLEA to Nycti on 2026-10-01
  ([ADR 0009](docs/architecture/adr/0009-rename-project-to-nycti.md)). The
  local folder `clea-desktop` was not renamed.
- Renamed: the package and binary (`nycti-windowd`), the library
  (`nycti_windowd`), the stderr prefix, the runtime directory
  (`$XDG_RUNTIME_DIR/nycti/`), the `status.service` value (`"nycti-windowd"`),
  and the live-test variables (`NYCTI_LIVE_ACTION_TEST`,
  `NYCTI_LIVE_MANAGER_TEST`). The old variables enable no test.
- Transition rule: stop any daemon started under the old name before starting
  the new one. There is no compatibility layer, and the new daemon does not
  remove the old `$XDG_RUNTIME_DIR/clea/` directory; remove it manually.
- ADRs 0001 to 0008 and earlier commit subjects keep the old name.
- The name of a future CLI is still open (to be closed by ADR 0010). No
  trademark search was done for the name Nycti.
- The test baseline did not change with the rename: **244 passed, 0 failed,
  3 ignored** at each rename commit; the ignored tests were not run.

## Repository and validated baseline

- Branch at validation: `feature/window-management-core`.
- Rust package: `nycti-windowd`, in `window-management/Cargo.toml`; there is no
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

After adding the `BackendError` messages, the daemon run logic, the executable,
and the process-level tests, package tests passed with **244 passed, 0 failed, 3
ignored**: 237 library tests (three `BackendError` tests and nine `daemon::run`
tests are new) and 7 process-level tests in `tests/daemon_process.rs`. The 19 new
tests were repeated 30 times, and the whole suite 30 times, without a failure and
with no daemon left running. `cargo check`, formatting verification, and
all-target Clippy also passed without warnings. The suite passed inside the
sandbox, and the ignored tests were not run. The process-level tests start the
real `nycti-windowd` binary as a child process with a cleared environment, a
private runtime directory, and a fake Hyprland socket serving the recorded
fixtures; they signal only their own child process, through `/usr/bin/kill`
(a dependency of the test environment), and never touch a real Hyprland session.
The automated tests never use a real Hyprland session.

Manual validation against a real Hyprland session: on 2026-09-30 the maintainer
ran the 10-step manual script on a real Hyprland session, using only read-only
protocol methods, and every step gave the expected result: startup, the
permissions of the runtime directory and the socket, the reads (`status`,
`get_default_mode`, `list_workspaces`, `list_windows`), `SIGTERM` and `SIGINT`
ending the daemon with code 0, recovery of a stale socket, a second daemon
exiting with code 2, environment failures exiting with code 1, and a stopped
Hyprland answering `compositor_unavailable`. This was run by the maintainer and
reported to the project; the project's tooling did not run it. Not validated
manually: the second signal and the forced exit (only the automated test covers
them), signals that arrive during startup, `SIGHUP`, and the case without a
focused window.

Manual validation of the renamed daemon: on 2026-10-01 the maintainer ran a
shorter manual validation of the renamed daemon on a real Hyprland session. The
2026-09-30 record above remains. This was run by the maintainer and reported to
the project; the project's tooling did not run it. Observed:

- A release build after `cargo clean`, in `/home/lhen/nycti`, produced
  `nycti-windowd`, and `clea-windowd` does not exist in `target/release`.
- The daemon printed `listening on /run/user/1000/nycti/window-management.sock`.
  The `nycti` directory has mode 0700 and the socket has mode 0600, both owned by
  the user. The directory `/run/user/1000/clea` does not exist.
- Read queries through `socat`: `status` returned `protocol_version` 1 and
  `service` `"nycti-windowd"`; `get_default_mode` returned `tiling`;
  `list_workspaces` returned 3 workspaces (`w:1` to `w:3`), all with effective
  mode `tiling`; `list_windows` returned 5 windows, all tiled.
- Shutdown by `SIGINT` (Ctrl+C): the report showed workers completed=4 failed=0
  panicked=0 refused=0, authority ok, and listener cleanup ok. The exit code was
  0, the socket was removed, and the `nycti` directory was left empty.
- Only read methods were exercised. The methods that change state were not
  exercised in this validation; automated tests cover them.

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
  `nycti-windowd-worker` that serves one accepted `UnixStream` through
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
  status Accepted): `Coordinator::start` takes the bound `UnixRuntimeListener`
  and the built service, starts the authority, and runs an accept thread named
  `nycti-windowd-accept` that owns the listener, the authority, and the worker
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
- `BackendError` implements `Display` and `std::error::Error`, with English
  messages that do not expose backend identities.
- Daemon executable ([ADR 0008](docs/architecture/adr/0008-window-management-daemon-signals.md),
  status Accepted): `nycti-windowd` registers `SIGTERM` and `SIGINT` through
  `signal-hook` before anything is bound, then `daemon::run_from_env` binds the
  service socket from `XDG_RUNTIME_DIR`, constructs the Hyprland backend from the
  session environment, builds the manager (default mode `Tiling`) and the
  service, and starts the coordinator. A dedicated signal thread reads the
  signals and calls `ShutdownHandle::request_shutdown`; no signal handler calls
  it. The first signal requests an orderly shutdown and a second forces an
  immediate exit with code 7. `main.rs` stays thin and `signals.rs` is the only
  code that knows the dependency; the library takes a `ShutdownSource` trait, so
  its tests inject a source. On exit the daemon prints a plain-text report on
  standard error and returns a code: 0 clean, 1 startup failure, 2 already
  running, 3 authority failed, 4 accept failed, 5 coordinator panicked, 6
  shutdown step failed, 7 forced exit. When the coordinator stops, the signal
  source is closed and the signal thread is joined. The compositor is not probed
  at startup.
- `signal-hook` 0.3.18 (`default-features = false`, feature `iterator`) is the one
  new dependency; it adds `signal-hook-registry` 1.4.5 and `libc` 0.2.174 to
  `Cargo.lock`, and no existing package changed version. Licenses, read from the
  manifests and license files of those packages in the local crates.io registry
  copy: `signal-hook` and `signal-hook-registry` Apache-2.0/MIT, `libc` MIT OR
  Apache-2.0. Both licenses are compatible with the GPL-3.0 of this project. The
  project's own code contains no `unsafe`.

## Not implemented

- Manual validation against a real Hyprland session of the second signal and the
  forced exit, of signals during startup, of `SIGHUP`, and of the case without a
  focused window. The forced exit only has its automated test.
- Supervision of the daemon process (for example a systemd user service).
- Automatic reaction to compositor events and reconciliation.
- Persistence of modes or external identities.
- Shell/Settings integration; those directories currently contain only READMEs.
- Packaging metadata and session integration.

Services and configuration also remain documentation-only component boundaries.
The library components do not yet form an operational desktop daemon.

## Next milestone

No next milestone is recorded yet. The manual validation of the daemon was done
on 2026-09-30, and ADR 0007 and ADR 0008 are Accepted. The open items are listed
under Not implemented and Known risks and limitations. The next implementation
step must be planned and authorized before it starts.

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
  detached, until the process exits. `main` waits on `Coordinator::wait` and ends
  the process with code 5 when that happens.
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
- The daemon is bound to the Hyprland instance resolved at startup
  (`HYPRLAND_INSTANCE_SIGNATURE`). If Hyprland restarts with another signature,
  the daemon must be restarted. The compositor is not probed at startup, so a
  stopped compositor shows up as `compositor_unavailable` on each request that
  needs it, while `status` keeps answering.
- The default mode `Tiling` is fixed in the daemon, with no configuration
  (provisional).
- A second signal during a stalled shutdown forces an immediate exit with code 7
  and runs no destructors, so the socket file stays on disk; the next start
  recovers it as a stale socket. `SIGHUP` keeps its default action (provisional),
  so closing the terminal ends the daemon without cleanup. If the signal thread
  dies (for example by panicking) before the daemon stops, nothing reads the
  signals any more: once `signal-hook` has installed its handling, `SIGTERM` and
  `SIGINT` are ignored instead of using the system default action, and only
  `SIGKILL` ends the daemon. The thread is joined only after the coordinator
  stops and its result is ignored, so this failure is not observed
  (provisional).
- Exit codes and the standard error text are provisional and promise no stable
  format. The process-level tests synchronize on the `listening on` and
  `shutdown requested` lines, so they are tied to that text, and they depend on
  `/usr/bin/kill` in the test environment.
- Not covered by tests: signals that arrive during startup, `SIGHUP`, and the
  failure to create the signal thread. The behavior of `signal-hook` with
  `SIGTERM` and `SIGINT` was validated manually on a real Hyprland session on
  2026-09-30, but neither the second signal nor `SIGHUP` was.
- Persistence, supervision, and compatibility with other Hyprland versions remain
  future work.

Architecture and contracts live in [docs/architecture](docs/architecture/).
