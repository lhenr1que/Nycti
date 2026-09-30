# ADR 0007: Window Management Daemon Coordinator, Failure Taxonomy, and Shutdown

- Status: Proposed
- Date: 2026-09-30

## Context

[ADR 0006](0006-window-management-daemon-execution.md) selects one accept/
lifecycle thread, one synchronous worker per connection, and one exclusive
service authority thread, using standard-library threads and channels. It
defers three things to the coordinator implementation: the means of waking a
thread blocked in `UnixListener::accept`, how connection workers are stopped,
and the concrete error vocabulary. It also leaves the response-line convention
between the transport and its request handler to the implementation.

This ADR records those implementation decisions. It does not change the
execution model of ADR 0006, and it does not modify that ADR.

## Decision

### Response line contract

A request handler returns exactly one complete response line terminated by LF.
The transport writes those bytes unchanged. It does not add, remove, or
validate the LF. The convention is documented and is not enforced by the code.
The client protocol already requires one LF-terminated line per response.

### Error taxonomy

Failure domains stay separate and are never merged:

- `TransportError` (`ReadFailed`, `WriteFailed`, `FlushFailed`): reading,
  writing, or flushing one connection.
- `UnixRuntimeError`: path, listener, accept, and stream-preparation failures.
  It never contains handler, protocol, or backend errors.
- Handler errors, carried by `ServeError<E>` and `UnixServeError<E>` as
  `Handler(E)`. For the daemon, `E` is `AuthorityError`.
- `AuthorityError`: `Unavailable` (the call was not admitted) and
  `ResponseLost` (the call was admitted but no response arrived, so its effect
  may have happened).
- `WorkerExit`: how a worker ended (`Completed`, `Transport`, `Preparation`,
  `Authority`).

A channel or authority failure is an internal daemon failure. It is never
converted into a protocol response such as `invalid_request` or
`internal_error`. The connection is closed without a response and the client
observes only EOF.

### Concurrent boundary bound

`WindowBackend` does not require `Send`. The `Send + 'static` bound applies only
where a value moves to another thread: `Authority::spawn`,
`Authority::spawn_with_exit_hook`, `Coordinator::start`, and the handler of a
worker.

### Closing worker connections

Each `WorkerHandle` keeps a duplicated descriptor (`try_clone`) of the
connection socket. `close_connection` calls `shutdown(Both)` on it, which wakes
a worker blocked reading from an idle client with EOF so that it can end
normally. No Rust state is shared; what is shared is the operating system's
duplicated descriptor. A `CloseOnDrop` guard in the worker also shuts the
socket down when the worker ends, including on panic, because the duplicate
would otherwise keep the socket open and the client would not observe EOF.

Cost (provisional): three to four descriptors per connection (the stream, the
control duplicate, the guard duplicate, and the runtime's reader clone). This
exists because `serve_unix_connection_with_handler` takes the stream by value.

Limitation: `close_connection` does not wake a worker that is waiting for the
authority inside a request. It ends after the authority answers or ends.

Rejected alternatives: an `Arc<AtomicBool>` flag with a read timeout (shared
state and polling); an `Arc<Mutex<Vec<UnixStream>>>` registry (shared state
without need).

### Stopping the accept loop

The coordinator owns one lifecycle event channel (`std::sync::mpsc`). A waker
sends an event and then connects to the daemon's own service socket, so the
blocked `accept` returns. The accept thread drains the channel after every
accept: a shutdown event or an authority-exit event ends the loop, and the
accepted stream is dropped without service. Shutdown requests come from a
cloneable `ShutdownHandle`, which is the point where signal handling will
connect later. No polling, timeout, or shared state is used.

The waker delivers the event before it connects, and it connects only if the
event was delivered. If the event cannot be delivered, the receiver is gone, the
coordinator has stopped, and the waker returns an error (`AlreadyStopped`)
without connecting.

The coordinator drops the receiver as soon as it leaves the accept loop,
before it closes or joins anything. After that point no waker connects to the
socket. This matters because the authority's exit hook also uses the waker: if
the hook could block on a `connect` while nobody accepts, `Authority::shutdown`
would never return.

Residual risk (limitation): the receiver can be dropped between a waker's
successful send and its connect. The connect then reaches a listener that no
longer accepts, and it blocks only if the listener's backlog is full. A full
backlog requires many simultaneous pending clients of the same user, and this
window cannot be closed without a non-blocking connect, which the standard
library does not provide for Unix sockets. There is no deterministic test for
this race.

This choice refines the means that ADR 0006 leaves to the coordinator
implementation. ADR 0006 also lists a "wakeup socket" among the things its
initial boundary does not select. The waker uses the service socket that
already exists and creates no separate wakeup socket, but this reading of
ADR 0006 is an interpretation and is recorded here explicitly.

Rejected alternatives: a non-blocking listener with sleeping (a polling timeout,
excluded by ADR 0006); `poll(2)` with a self-pipe or `eventfd` (a new dependency
and a separate wakeup channel); `shutdown(2)` on the listener descriptor (a new
dependency and platform-specific behavior); abandoning the accept thread (leaks
the listener owner, which ADR 0006 forbids).

### Authority failure

The authority thread runs an exit hook from a drop guard, including while
unwinding from a panic. The coordinator's hook sends an authority-exit event
through the waker. Because the coordinator holds an authority client, an
authority exit while the loop runs is a failure. The coordinator then stops
admitting connections, runs the shutdown sequence, and reports the failure. It
never creates another service or authority.

The hook must not panic. It ignores every error, and it uses no `unwrap` or
`expect`, because it runs in `Drop` during unwinding, where a second panic would
abort the process. As an extra precaution the drop guard also contains a
panicking hook instead of letting it abort the process.

### Shutdown sequence

1. Leave the accept loop and drop the event receiver.
2. Close every worker connection.
3. Join every worker.
4. Drop the coordinator's authority client and call `Authority::shutdown`.
5. Clean up the listener by recorded device and inode.

Every step runs even if an earlier one fails, and each result is reported. The
listener is cleaned last so that the socket path stays owned until the daemon
has stopped, which prevents a second daemon from starting while this one drains.
Clients that connect during the drain wait in the backlog and observe EOF when
the listener closes.

### Coordinator failure

If the coordinator thread panics, `Coordinator::wait` returns
`CoordinatorError::Panicked`. This is fatal for the process: the workers and the
authority are left running, detached, until the process exits. A future
`main.rs` must observe `Coordinator::is_finished` or call `Coordinator::wait`
and end the process.

### Accept errors and worker spawn failures

An accept error is fatal and leads to an orderly shutdown.
`UnixRuntimeError::AcceptFailed` does not keep the `io::ErrorKind`, so transient
and fatal errors cannot be told apart. Improving this requires preserving the
`io::ErrorKind` in the Unix runtime. A failure to create a worker closes that
connection and is counted; the coordinator never serves a connection on its own
thread.

## Consequences

- One authority, one listener owner, and a defined shutdown order.
- Authority failure is detected when it happens and is never hidden behind an
  open listener.
- Signal handling attaches to `ShutdownHandle` without further change.

### Test coverage

Deterministic tests cover the waker rule: no connect when the receiver is gone,
connect after a successful send, and an error when the socket is missing. The
race described under residual risk has no deterministic test. The accept error
path, worker creation failure, coordinator creation failure, and a panic of the
coordinator thread are also not covered by tests.

### Debts and limitations (provisional)

- No connection limit, line-size limit, idle timeout, or logging.
- Finished workers hold a descriptor until the next accept or shutdown.
- Accept errors are not classified.
- A missing or replaced socket file prevents waking the accept loop.
- Shutdown waits for workers blocked on a slow authority.
- Three to four descriptors per connection.
- The waker's connect can block in the residual race described above.

## Out of scope

Signal handling, systemd integration, a functional `main.rs`, connection limits,
event subscriptions, persistence, and reconciliation.
