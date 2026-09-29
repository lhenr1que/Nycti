# ADR 0006: Window Management Daemon Execution Model

- Status: Accepted
- Date: 2026-09-28

## Context

[ADR 0005](0005-window-management-runtime.md) establishes `clea-windowd` as the
single per-user Window Management authority, but deliberately leaves its
threading and multi-client scheduling model open. The
[Unix runtime specification](../window-management-unix-runtime.md) now provides
a secure `UnixRuntimeListener`, accepts a `UnixStream`, and connects one stream
to the existing synchronous JSON Lines transport.

The current transport serves a connection through conceptually:

```text
UnixStream
    |
    v
serve_connection(..., &mut WindowManagementService)
```

`serve_connection` keeps mutable access to the service until that connection
reaches EOF or fails. A whole-connection synchronous daemon would therefore do:

```text
accept A
serve A until EOF
accept B
```

A client could keep connection A open indefinitely and prevent B from being
served, even while A sends no requests.

Creating one service per connection is not an acceptable solution.
`WindowManagementService` owns state that must be shared consistently across
all clients, including:

- the default workspace mode;
- explicit workspace modes;
- the external identity registry;
- the `WindowManager`; and
- backend state.

Multiple independent services would create multiple authorities and violate
the runtime architecture. The daemon needs concurrent connections while
preserving exactly one logical service authority.

## Decision

### Execution model

The initial daemon will use only synchronous standard-library threads and
channels:

- one accept/lifecycle thread;
- one synchronous connection worker thread for each accepted connection; and
- one authority thread that exclusively owns the single
  `WindowManagementService<HyprlandBackend>`.

The conceptual flow is:

```text
UnixRuntimeListener
        |
        +---- UnixStream A -> connection worker A --+
        |                                            |
        +---- UnixStream B -> connection worker B --+--> request channel
        |                                            |         |
        +---- UnixStream C -> connection worker C --+         v
                                                    single
                                                    service
                                                   authority
                                                       |
                                                       v
                                           WindowManagementService
                                                       |
                                                       v
                                                WindowManager
                                                       |
                                                       v
                                                HyprlandBackend
```

The accept/lifecycle thread and connection workers never receive direct access
to the service, manager, or backend. They receive only the channel handle needed
to submit complete requests to the authority.

### Single service authority

The authority thread is the only owner of one `WindowManagementService<B>` and,
by transitive ownership, the only owner of:

- `WindowManager<B>`;
- `ExternalIdentityRegistry`;
- default mode state;
- explicit workspace mode state; and
- the backend instance.

The initial implementation must not:

- wrap `WindowManagementService` in `Arc<Mutex<_>>`;
- clone the backend;
- create a service per connection; or
- create a service per request.

The authority receives and processes one request at a time. It does not begin
the next service operation until the current operation has completed and its
response has been handed off to the originating worker. This creates a total
order for all service reads, mutations, identity allocations, and backend
operations during one daemon lifetime.

### Backend ownership and `Send`

`HyprlandBackend` is created exactly once for a daemon instance. It is moved
through `WindowManager` and `WindowManagementService` into the authority thread
and is never shared with connection workers.

Moving a generic service into the authority thread may require an implementation
boundary equivalent to:

```text
B: WindowBackend + Send + 'static
```

That bound belongs on the concurrent daemon/coordinator boundary that actually
moves `B` between threads. This decision does not add `Send` as a global
supertrait of `WindowBackend`; core and service tests that do not cross a thread
boundary need not satisfy a concurrency requirement they do not use.

### Request unit and service calls

Each connection worker owns the I/O and JSON Lines framing lifecycle of one
accepted `UnixStream`. For every complete request line, it submits one
conceptual `ServiceCall` containing:

- the complete request bytes, without interpreting the JSON; and
- a dedicated response sender for that request.

The flow for one request is:

```text
complete JSON request bytes
        |
        v
ServiceCall
        |
        v
authority
        |
        v
WindowManagementService::handle_json_bytes(...)
        |
        v
serialized protocol response line
        |
        v
originating connection worker
        |
        v
client
```

The authority handles request bytes, not `UnixStream`, `BufReader`, filesystem
paths, or connection lifecycle. It does not read from or write to client
sockets.

### Channel model and backpressure

The first implementation will use `std::sync::mpsc` only. It will not add
Tokio, crossbeam, or another channel/runtime dependency.

Workers submit `ServiceCall` values through one central rendezvous channel,
conceptually:

```text
std::sync::mpsc::sync_channel(0)
```

Zero capacity is intentional:

- there is no arbitrarily growing central request queue;
- there is no unexplained capacity constant;
- a worker waits until the authority is ready to admit its request; and
- only one service request is admitted for processing at a time.

Each call uses a dedicated one-response standard-library channel. The response
handoff must not wait for the client to read from its socket. An unbounded
`std::sync::mpsc::channel()` is suitable because the sender produces exactly one
response for that call; equivalently, another one-shot design may be used if the
authority can hand off the response without becoming blocked on client I/O.

After handoff, the worker alone may block while writing or flushing the response
to its client. If the response receiver has disappeared, the authority discards
the response and continues processing later calls.

### Framing remains centralized in transport

Workers must not implement a second JSON Lines framing loop. In particular,
daemon code must not duplicate:

- `read_until(b'\n')` behavior;
- clean EOF handling;
- incomplete final-line handling;
- exact response LF behavior; or
- continuation after an invalid complete request line.

The transport module will be refactored around one reusable framing primitive
that invokes a request handler for each complete line, conceptually similar to:

```text
serve_connection_with_handler(reader, writer, handler)
```

The concrete name and trait shape are not normative. The existing
`serve_connection(..., &mut WindowManagementService)` may remain as a wrapper
or convenience adapter over the same primitive. There must still be only one
framing implementation.

The handler-capable transport must preserve separate failure domains:

- `TransportError` for read, write, and flush failures; and
- a handler/authority error for an unavailable authority or closed channel.

A channel or authority failure is an internal daemon/runtime failure. It must
not be converted automatically into `invalid_request`, `internal_error`, or
another protocol response that would incorrectly attribute the failure to the
client or claim normal protocol processing.

The transport refactor must preserve all current framing behavior and tests,
including:

- invalid JSON produces a response and the connection continues;
- invalid UTF-8 is handled as a protocol error and the connection continues;
- an empty line is a complete invalid request;
- clean EOF succeeds without a response;
- an incomplete final line is discarded;
- each complete request receives one response;
- responses have exact LF framing;
- multiple requests are supported; and
- sequential requests on one connection observe shared service state.

### Ordering semantics

Within one connection:

- at most one request from that connection is admitted or processed at a time;
- the worker waits for the response before advancing to the next request line;
- responses remain in the same order as requests; and
- protocol v1 never produces out-of-order responses.

Between different connections:

- there is no protocol-level ordering guarantee;
- effective order is the order in which the authority receives and admits
  calls from the rendezvous channel; and
- exact fairness among competing workers is not promised.

The daemon does not assign priority to Shell, Settings, CLI, or any other client
in this version.

### Idle and slow clients

A connected client that has not submitted a complete request occupies only its
connection worker and socket. It does not hold the service, block the authority,
or prevent other workers from submitting requests.

A client that is slow to read a response may block its own worker in socket
write or flush. It must not block the authority after the authority has handed
the response bytes to that worker's response channel, and it must not block
other connection workers.

### Disconnects and side effects

Admission to the authority is the boundary after which processing may have
effects even if the originating client disconnects before receiving its
response. Once admitted, the authority processes the request normally.

The daemon does not promise:

- exactly-once response delivery;
- transactional delivery coupled to client receipt;
- rollback when a client disconnects; or
- automatic retry.

If the authority completes an operation but cannot return its response because
the worker or response receiver disappeared, it discards that response and
continues. It does not undo service state or backend actions that already
occurred. Clients, especially callers of mutating methods, must not interpret a
missing response as proof that the operation did not occur.

### Protocol errors

Complete requests continue to pass through
`WindowManagementService::handle_json_bytes`. Invalid JSON,
`unsupported_version`, `invalid_params`, `unknown_workspace`, `action_failed`,
and other defined protocol/backend outcomes remain normal `ProtocolResponse`
values.

Such responses do not terminate the connection worker, authority, or daemon.
The worker writes the response and continues with the next complete request on
that connection unless its transport itself fails.

### Worker and connection failures

A read, write, flush, EOF, or disconnect condition belongs to one connection.
It ends or completes only that connection worker and does not terminate the
authority or other workers. The daemon performs no automatic request or
connection retry.

Worker creation may use `std::thread::Builder` so spawn failure can be reported.
If a worker cannot be created:

- the newly accepted stream is closed;
- the existing authority remains alive;
- existing workers remain unaffected; and
- the daemon does not silently serve that connection synchronously on the
  accept thread.

### Fatal authority lifecycle

The authority's continued existence is a requirement for the daemon to be
operational. If the authority terminates unexpectedly or the central request
channel becomes permanently disconnected, the coordinator enters a fatal
state. It must not construct another `WindowManagementService`, start a
replacement authority, or deliberately continue accepting and serving new
connections. The failure is reported to the higher process/lifecycle layer.

Existing workers that discover the closed authority channel terminate only
their own connections with an internal daemon/runtime error. They do not invoke
the service locally, create a service or backend, or attempt another fallback.
An unexpected authority exit or panic remains a fatal daemon/runtime failure,
not part of the client protocol.

The coordinator must retain an explicit way to observe authority termination,
for example through ownership of its `JoinHandle` or an equivalent
standard-library lifecycle mechanism. This requirement does not select a
specific primitive for waking a thread blocked in `UnixListener::accept`.
Blocking `accept` has no promise of instantaneous authority-failure detection in
this decision. Authority failure is semantically fatal, and after the
coordinator observes it no new connection may be admitted; the concrete means
of interrupting or waking blocked accept, if needed, is deferred to the
coordinator implementation.

### Accept and lifecycle ownership

The accept/lifecycle thread owns the `UnixRuntimeListener`. It:

- accepts `UnixStream` connections;
- creates one connection worker for each accepted stream;
- gives each worker only a clone of the authority request sender; and
- never constructs or accesses `WindowManagementService`, `WindowManager`, or
  `WindowBackend` in the accept loop.

The concrete daemon coordinator will expose lifecycle failures, including
worker spawn failures and fatal authority failure, to its caller. This ADR does
not define a protocol response for those failures. Once fatal authority failure
has been observed, the accept/lifecycle role stops admitting new connections
rather than presenting a listener that appears operational without its service
authority.

### Resource model

One blocking worker thread per connection is acceptable for the initial daemon
because:

- the Unix socket is local;
- socket mode `0600` and parent mode `0700` restrict access to the user;
- blocking `std::os::unix::net` remains simple; and
- no demonstrated requirement yet justifies an async runtime.

This version defines no normative maximum number of simultaneous connections or
worker threads. Connection limiting may be added later as operational hardening
without changing the single-authority model.

### Startup order

The future daemon startup sequence is:

1. bind `UnixRuntimeListener` through `XDG_RUNTIME_DIR`;
2. construct one `HyprlandBackend`;
3. construct `WindowManager` with the initial default mode;
4. construct one `WindowManagementService`;
5. move that service into the authority thread; and
6. begin accept and connection-worker serving.

If any stage before the accept loop fails, startup fails. Ownership of the
`UnixRuntimeListener` provides safe socket cleanup according to the Unix runtime
specification; startup does not leave a deliberately live listener without a
service authority.

### Initial mode and persistence boundary

The initial default for the first daemon implementation is:

```text
WorkspaceMode::Tiling
```

Persistence remains out of scope. Therefore, after a daemon restart:

- the default returns to `tiling`;
- explicit workspace overrides are lost; and
- a new external identity registry is created.

This is the behavior of the initial in-memory implementation, not a permanent
policy requirement for all future CLEA versions. A future persistence decision
may change restart behavior without changing the single-authority execution
model.

### Shared external identity lifecycle

Because all clients use one `WindowManagementService`, they share one
`ExternalIdentityRegistry`. During one daemon lifetime:

- a workspace receives the same external token regardless of which client
  lists it; and
- a window receives the same external token regardless of which client lists
  it.

Tokens retain only the lifetime guarantees defined by protocol v1. The initial
registry is not persisted across daemon restart.

### Shutdown boundary

This decision does not select signal handling or service supervision. The first
coordinator implementation must support controlled shutdown in tests through
explicit ownership of the resources needed to:

- stop admitting new connections;
- allow authority sender clones to be dropped as connection workers terminate;
- observe and join the authority thread; and
- perform identity-safe cleanup of the owned `UnixRuntimeListener`.

The concrete coordination API is deferred, but shutdown must not depend on
abandoning the authority thread or leaking the listener owner. This initial
boundary will not add or select:

- a `ctrlc` crate;
- `signal-hook`;
- direct `libc` signal handling;
- systemd integration;
- an async runtime, polling timeout, or wakeup socket for interrupting blocked
  accept; or
- a protocol `shutdown` method.

Graceful `SIGTERM`/`SIGINT`, session supervision, and systemd lifecycle remain
future work. Abrupt process termination may leave a socket pathname; the Unix
runtime's identity-safe stale-socket recovery remains responsible for that
case.

This ADR decides the model that `main.rs` will eventually use, but does not make
`main.rs` functional. The coordinator and authority must first be implemented
and tested independently of the binary entry point.

## Alternatives considered

### Single-thread whole-connection serving

Rejected because one persistent or idle connection would monopolize the daemon
until EOF and prevent other clients from being served.

### `WindowManagementService` per connection

Rejected because each connection would obtain independent default modes,
explicit modes, external identities, manager state, and backend state. This
would divide the authority selected by ADR 0005 and make client observations
depend on which connection handled them.

### `Arc<Mutex<WindowManagementService<_>>>`

Not selected for the first implementation. Locking once per request could
preserve one service instance, but:

- authority ownership becomes less explicit;
- lock lifetime and poisoning become part of the design;
- client-I/O code can accidentally retain the lock while reading or writing;
  and
- a dedicated owner thread provides a clearer boundary around service and
  backend access.

This alternative is not prohibited forever. It may be reconsidered if a future
requirement provides a concrete reason and preserves the same single logical
authority and request-level I/O separation.

### Tokio or another async runtime

Deferred because the local daemon has no demonstrated async-runtime requirement
and standard blocking threads plus channels satisfy the initial multi-client
needs without a new dependency.

### Custom `poll`/`select` event loop

Not selected because it adds socket-state, buffering, wakeup, and scheduling
complexity before there is evidence that the thread-per-connection model is
insufficient.

## Consequences

### Advantages

- Exactly one service remains the Window Management authority.
- Multiple clients can keep connections open concurrently.
- An idle client does not block service requests from other clients.
- All service operations and backend access are serialized.
- Workers never own or share the backend.
- No runtime or channel dependency is added beyond the Rust standard library.
- Per-connection request and response ordering remains simple and explicit.
- The coordinator and authority can be tested with `FakeBackend` and local or
  in-memory transports without a real Hyprland session.

### Costs and limitations

- Every live connection consumes one operating-system thread and its resources.
- Thread creation, scheduling, and context switching have a cost.
- The standard-library rendezvous channel provides no promised fairness among
  workers.
- The authority is deliberately serial and may become a throughput bottleneck.
- One slow backend operation blocks all later service operations, though not
  connection reads already waiting in their own workers.
- No connection or worker limit is defined yet.
- Graceful process signal handling and supervision remain deferred.
- Coordinator code must distinguish transport, worker, channel, and fatal
  authority failures without turning them into misleading protocol errors.

## Future test requirements

The execution model must be testable with
`WindowManagementService<FakeBackend>` and no real Hyprland session. Future
tests must demonstrate at least:

1. exactly one service instance is used;
2. two connection workers share the default mode;
3. `set_default_mode` by client A is observed by `get_default_mode` from B;
4. an explicit workspace mode set by A is observed by B;
5. an external workspace token emitted to A remains the same for B;
6. an external window token emitted to A remains the same for B;
7. client A may remain connected and idle without blocking requests from B;
8. an error or EOF on A does not terminate B;
9. a write failure on A does not terminate the authority;
10. invalid JSON on A does not affect B;
11. a protocol error on A does not affect B;
12. two concurrent mutations are processed sequentially by the authority;
13. requests from one connection retain their order;
14. each response returns to the worker that originated its request;
15. disconnect after authority admission may leave the request side effect
    applied;
16. failure to return a response to a departed worker does not terminate the
    authority;
17. a closed authority channel terminates workers without creating a new
    service;
18. no worker accesses `WindowBackend` directly;
19. no worker contains a clone of `WindowManagementService`;
20. no connection test requires a real Hyprland session;
21. the existing framing implementation remains reused; and
22. no async runtime is required.

Tests for ordering and disconnect boundaries should use controlled channels and
fixtures rather than sleeps or timing assumptions.

## Deliberately out of scope

This decision does not define or implement:

- Tokio or another async runtime;
- D-Bus;
- systemd integration or socket activation;
- signal handling;
- persistence;
- connection or worker limits;
- authentication beyond the existing Unix filesystem boundary;
- client or method priorities;
- exact fairness among connections;
- event subscriptions or notifications;
- Hyprland `socket2` event integration;
- reconciliation;
- minimize, maximize, restore, or geometry policy; or
- a functional `main.rs`.

## Acceptance criteria

An implementation conforms to this decision when it:

1. creates exactly one `WindowManagementService` authority for the daemon;
2. allows multiple connections so an idle connection does not block the entire
   daemon;
3. gives exclusive service, manager, registry, and backend ownership to one
   authority thread;
4. serializes service operations one at a time in authority admission order;
5. preserves request and response order within each connection;
6. promises no cross-client order beyond actual authority admission and no
   exact fairness;
7. reuses one transport framing implementation rather than duplicating JSON
   Lines behavior in workers;
8. keeps `TransportError` distinct from handler and authority failures;
9. never exposes the service or backend to connection workers;
10. performs no automatic retry or rollback when a client disconnects;
11. permits admitted operations to take effect even when their response cannot
    be delivered;
12. treats an absent or terminated authority as meaning that the daemon is not
    operational;
13. does not create a replacement service or authority when the original
    authority exits or panics;
14. stops admitting new connections after fatal authority failure has been
    observed by the coordinator;
15. makes no promise of instantaneous authority-failure detection while blocked
    in `UnixListener::accept` and leaves the concrete wakeup mechanism to the
    coordinator implementation;
16. retains lifecycle ownership that allows tests to observe and join the
    authority, stop admissions, close authority senders, and safely clean up the
    listener;
17. uses only standard-library threads and channels for the initial model;
18. applies any required `Send + 'static` bound at the concurrent runtime
    boundary rather than globally changing `WindowBackend` without need;
19. starts the in-memory daemon with `WorkspaceMode::Tiling` and documents the
    loss of modes and external identities on restart;
20. supports controlled test shutdown through ownership/channel lifecycle
    without selecting production signal handling; and
21. passes multi-client tests with `FakeBackend` and no real Hyprland session.

No code is introduced by this ADR.
