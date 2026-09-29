# CLEA Window Management Unix Runtime

## Status and scope

This document specifies the local Unix runtime adapter for `clea-windowd`. It
narrows the runtime and socket decisions in
[ADR 0005](adr/0005-window-management-runtime.md) and connects the Unix socket
boundary to the existing synchronous JSON Lines transport.

The adapter is responsible only for:

- resolving the per-user runtime root;
- preparing the CLEA runtime directory;
- classifying an existing socket path;
- recovering a narrowly defined stale socket;
- binding and protecting a `UnixListener`;
- accepting a `UnixStream`;
- preparing an accepted stream for the existing transport; and
- safely cleaning up the socket created by the current process.

It does not redefine the client protocol, JSON Lines framing, service methods,
Window Management policy, or compositor integration. No code is introduced by
this specification.

## Architectural boundary

The dependency flow is:

```text
Shell / Settings / CLI
        |
        v
Unix domain socket
        |
        v
Unix runtime adapter
        |
        v
serve_connection(...)
        |
        v
WindowManagementService
        |
        v
WindowManager
        |
        v
WindowBackend
```

The Unix runtime adapter owns local path, filesystem, listener, accepted-stream,
and socket-lifecycle concerns. It does not interpret or depend on:

- protocol methods or parameters;
- `WorkspaceMode`;
- `WorkspaceId` or `WindowId`;
- `BackendError`;
- window placement policy; or
- Hyprland types, sockets, commands, or state.

JSON validation and protocol responses remain below `serve_connection`. The
runtime adapter must not duplicate framing, parse JSON, construct protocol
responses, or call `WindowManager` or `WindowBackend` directly.

## Normative paths

The production runtime root is taken only from `XDG_RUNTIME_DIR`. The CLEA
runtime directory and Window Management socket are:

```text
$XDG_RUNTIME_DIR/clea
$XDG_RUNTIME_DIR/clea/window-management.sock
```

The daemon must not use any of the following as a fallback or substitute:

- `/tmp`;
- a hardcoded user ID;
- `HOME`;
- a relative path; or
- a Hyprland socket or runtime path.

Test-only explicit roots are addressed under [Testability](#testability). They
do not change production path resolution.

## Resolving `XDG_RUNTIME_DIR`

Production startup resolves `XDG_RUNTIME_DIR` from the current process
environment. Before preparing the child `clea` directory, the runtime root must
satisfy all of these requirements:

1. the environment value is present;
2. the value is non-empty;
3. the path is absolute;
4. the path exists;
5. no-follow inspection of the final component, conceptually through
   `symlink_metadata`, proves that it is a real directory and not a symbolic
   link; and
6. its Unix permission mode is exactly `0700`.

There is no fallback. The daemon must not normalize or otherwise change the
permissions of `XDG_RUNTIME_DIR`. A runtime root with a mode other than `0700`
causes startup to fail.

The resolver must not require UTF-8 when the platform path representation can
carry the environment value losslessly. It must not canonicalize the complete
future path as a prerequisite for startup: `$XDG_RUNTIME_DIR/clea` is allowed
not to exist yet and is created by the next stage. Validation applies directly
to the supplied runtime-root path without following a symlink at its final
component.

This first version does not require an additional explicit UID-owner check when
that would require a new dependency or platform API. Ownership enforcement may
be refined later without weakening the required no-follow type and `0700` mode
validation.

Error classification follows a deterministic order. An absent or empty
environment value is `RuntimeDirUnavailable`. A relative path is
`InvalidRuntimeDir` without attempting metadata inspection. During no-follow
inspection, `NotFound` is `InvalidRuntimeDir`, while another operational error
that prevents obtaining the required metadata is `RuntimeDirUnavailable`.
Successfully obtained metadata that identifies a symlink, non-directory, or
mode other than `0700` is `InvalidRuntimeDir`.

An internal validated-root type may be used to prevent later code from joining
the socket name onto an unchecked or relative path. Such a type is an
implementation detail and does not make arbitrary production roots part of the
daemon's public interface.

## Preparing the CLEA runtime directory

The daemon uses the direct child named `clea` under the validated runtime root.
It must inspect this path without following its final component, conceptually
using `symlink_metadata`.

### Path absent

On Unix, create the `clea` directory with requested mode `0700` from the start,
conceptually using:

```text
std::fs::DirBuilder
std::os::unix::fs::DirBuilderExt::mode(0o700)
```

The process umask may remove permission bits, but it must not cause the
directory to be created with permissions broader than `0700`. The implementation
must not create the directory with default permissions and rely only on a later
chmod.

After creation, inspect the path again without following its final component,
confirm that it is the expected real directory, normalize its mode to `0700` if
the umask removed bits, and verify the final mode is exactly `0700`.

A create-versus-create race may report that the path already exists; in that
case the implementation must inspect the resulting path under the same rules as
an existing path rather than assuming it is safe.

### Existing real directory

Continue only when the final `clea` path is a real directory and not a symlink.
Its permission bits must be set to `0700`. If it already has another mode, the
daemon may normalize it to `0700`, then verify that the expected real directory
still occupies the path.

### Existing unsafe object

Startup fails if `clea` is any of the following:

- a symbolic link, including one whose target is a directory;
- a regular file;
- a Unix socket;
- a FIFO;
- a device; or
- any other non-directory object.

The daemon must not remove or replace such an object automatically. It must not
silently follow a symlink at the `clea` path.

### Directory permissions

The final mode of the real CLEA runtime directory is:

```text
0700
```

This directory is the primary barrier preventing other users from reaching the
socket path during setup, including the short interval between socket creation
and explicit socket permission normalization. The daemon does not modify the
mode of `XDG_RUNTIME_DIR` itself.

Requesting `0700` at directory creation eliminates a potentially permissive
create-to-chmod interval for `clea`. This differs from the socket itself: the
first socket implementation may still rely on the already verified `0700`
parent directory during its short bind-to-chmod interval. No `libc` dependency
is needed for either rule.

## Inspecting an existing socket path

Before bind, the adapter inspects
`$XDG_RUNTIME_DIR/clea/window-management.sock` without following a symbolic
link, conceptually using `symlink_metadata`.

### Path absent

Proceed to one bind attempt.

### Path exists but is not a real Unix socket

Fail startup with `UnsafeSocketPath`. The daemon must not automatically remove,
replace, truncate, or follow:

- a regular file;
- a symbolic link, even one targeting a socket;
- a directory;
- a FIFO;
- a device; or
- an unknown object.

This rule prevents stale-socket recovery from becoming a generic deletion
mechanism for unexpected user data.

### Path is a real Unix socket

Attempt `UnixStream::connect` to the socket path solely to distinguish an active
listener from the one explicitly recoverable stale state. This probe sends no
protocol bytes. A successful connection is closed immediately.

## Active daemon detection

If the connect probe succeeds:

- another listener is considered active;
- startup fails with `AlreadyRunning`; and
- the existing socket path is not removed.

The runtime adapter does not send `status` or any other request. Listener
reachability is sufficient for this startup check and avoids coupling runtime
ownership to protocol behavior.

## Stale socket detection and recovery

The first implementation classifies a path as stale only when both conditions
hold:

1. no-follow metadata proved that the path was a real Unix socket; and
2. `UnixStream::connect` failed specifically with `ConnectionRefused`.

Before removal, the adapter must inspect the path again without following
symlinks and confirm that it is still the same socket observed before the
connect probe. On Unix, device and inode are suitable identity fields. If the
path changed to another object or socket, the adapter must not remove it and
startup fails safely.

If the same stale socket is still present, remove it and make one normal bind
attempt. Failure to remove that confirmed stale socket is
`StaleSocketCleanupFailed`, except that `NotFound` caused by a race may proceed
to bind.

No other connect error means stale. In particular, `PermissionDenied`,
`InvalidInput`, and unexpected operating-system errors fail startup without
removing the path. An implementation may classify these as
`ExistingSocketCheckFailed`; it must not collapse them into stale recovery.

If the connect probe reports `NotFound` because the path disappeared between
metadata and connect, proceed to the normal bind attempt. The disappearance is
not itself corruption. Bind remains the final authority over whether the path
can be acquired.

## Startup race and bind authority

The sequence:

```text
inspect
    |
    v
optional confirmed-stale cleanup
    |
    v
bind
```

is not atomic. Another process may win the path after inspection or stale
cleanup and before bind. The first implementation performs exactly one bind
attempt. If another process wins, bind fails with `BindFailed`.

After bind failure, the adapter must not retry automatically, repeat stale
detection, or remove the path again. In particular, it must not delete a socket
that may have been created by the process that won the race.

## Binding and socket permissions

The adapter binds only after the CLEA directory has been verified as a real
directory with mode `0700` and the existing-path procedure has completed.

After successful `UnixListener::bind`, the adapter must:

1. inspect the created path without following symlinks;
2. verify that it is a Unix socket;
3. record its cleanup identity, including device and inode on Unix;
4. set the socket permission bits to `0600`; and
5. verify that the expected socket still occupies the path with mode `0600`.

The normative socket mode is:

```text
0600
```

Rust `std` does not provide a portable process-umask guard, and the first
implementation must not add `libc` solely to manipulate umask. The already
verified `0700` parent directory protects the socket during the small interval
between bind and chmod. Explicit permission normalization after bind remains
required.

If socket permission setup fails, startup fails with
`SocketPermissionFailed`. Cleanup may remove only the socket whose recorded
identity was created by this bind; it must apply the same ownership check as
normal cleanup.

## Listener ownership

The future implementation should represent a successful bind with one owner
conceptually similar to `UnixRuntimeListener` or `BoundUnixListener`. The
concrete Rust name is not normative. The owner contains at least:

- the `UnixListener`;
- the full socket path; and
- the recorded identity needed for safe cleanup.

The owner prevents the listener and its cleanup evidence from being separated
accidentally. It may expose a focused `accept` or `accept_connection` operation
that returns an accepted `UnixStream`. It must not own a second
`WindowManagementService`, parse protocol messages, or contain compositor
state.

## Safe socket cleanup

Normal shutdown should offer explicit cleanup through the listener owner. A
best-effort `Drop` may provide a final safety net, but `Drop` cannot report
errors and must never perform unconditional `remove_file(socket_path)`.

Immediately before removal, cleanup inspects the path without following
symlinks:

- if the path is the same Unix socket identity recorded after bind, remove it;
- if the path does not exist, cleanup is a successful no-op;
- if the path is a symlink, non-socket object, different device/inode, or a
  different socket, do not remove it; and
- if identity inspection or removal of the confirmed owned socket fails for an
  unexpected reason, explicit cleanup returns `SocketCleanupFailed`.

This identity check prevents an old process from intentionally removing a path
that now belongs to a replacement listener or contains another object. Cleanup
must use the recorded identity, not merely the expected pathname or the fact
that some socket exists there.

In a crash, `SIGKILL`, or power loss, cleanup is not guaranteed. This is
expected. The narrowly defined stale-socket startup procedure exists to recover
the socket left by such termination.

## Accepting and preparing a connection

The listener owner may provide an operation equivalent to:

```text
accept() -> UnixStream
```

Accept failures are `AcceptFailed`. An accepted stream is adapted to the
existing generic transport without moving framing into the Unix runtime:

```text
UnixStream
    |
    v
BufReader<UnixStream> + writer view/handle
    |
    v
serve_connection(..., &mut WindowManagementService)
```

For a synchronous `UnixStream`, one straightforward implementation may clone
the stream handle, use one handle inside `BufReader`, and use the other as the
writer. Failure to create the required reader/writer arrangement is
`ConnectionPreparationFailed`. The exact arrangement is not normative as long
as it supplies `BufRead` and `Write` to `serve_connection` without implementing
another framing loop.

`TransportError` remains the error vocabulary for read, write, and flush while
serving an established connection. `UnixRuntimeError` covers path, listener,
accept, and stream-preparation failures. Protocol and backend failures continue
to become protocol responses through the existing service. These error domains
must not be merged.

The adapter also provides a handler-capable variant,
`serve_unix_connection_with_handler`, which prepares the same accepted-stream
arrangement and supplies it to the transport's handler-based framing primitive,
`serve_connection_with_handler`, instead of to a `WindowManagementService`. The
handler receives each complete request line without its LF and returns one
complete response line terminated by LF, which is written unchanged; that LF is
a documented convention and is not validated. Stream preparation is implemented
once and shared by `serve_unix_connection` and the handler variant. The adapter
does not call the service, manager, or backend on the handler path, and
`serve_unix_connection` keeps its existing signature and behavior.

If the handler fails, no response is written for that request, serving stops,
and the failure is returned as `UnixServeError::Handler`. Dropping the stream
closes the connection, so the client observes only EOF and no protocol response.
A handler failure is not converted into a protocol error or a `UnixRuntimeError`.
If the stream cannot be prepared, the handler is not called.

## Concurrency and service authority are specified separately

This specification covers the Unix adapter, not daemon coordination.
[ADR 0006](adr/0006-window-management-daemon-execution.md) now selects one
accept/lifecycle role, one synchronous worker per connection, and one exclusive
service authority thread, using standard-library channels with a central
zero-capacity rendezvous channel. No exact fairness between clients is promised.
The coordinator remains unimplemented; the existing adapter still serves one
accepted stream through the generic framing layer.

`WindowManagementService` contains mutable authoritative state, including the
default mode, explicit workspace modes, external identity registry, and owned
backend state through `WindowManager`. The multi-client implementation must
preserve one logical authority. It must not create one service per connection
or request.

The first Unix-adapter implementation may therefore stop at:

- secure bind and ownership;
- acceptance of one connection; and
- adaptation of one accepted stream to `serve_connection`.

It need not introduce a daemon-wide accept loop. `main.rs` must remain without a
complete functional daemon while this adapter is implemented and tested in
isolation.

## Runtime error model

The Unix adapter uses a small `UnixRuntimeError` separate from
`TransportError`. Concrete Rust variant names may be refined, but the following
categories and distinctions are required:

| Category | Meaning |
|---|---|
| `RuntimeDirUnavailable` | `XDG_RUNTIME_DIR` is absent or empty, or an operational failure prevents obtaining required metadata for it. |
| `InvalidRuntimeDir` | The resolved path is relative or nonexistent, or no-follow inspection shows that its final component is a symlink, is not a directory, has a mode other than `0700`, or otherwise structurally fails the runtime-root requirements. |
| `RuntimeDirectorySetupFailed` | Creating, inspecting, or setting required permissions on the CLEA directory failed operationally. |
| `UnsafeRuntimeDirectory` | The `clea` path is a symlink or another unexpected non-directory object. |
| `UnsafeSocketPath` | The socket pathname contains an existing non-socket object, symlink, or changed identity that must not be removed. |
| `AlreadyRunning` | The existing Unix socket accepted a connect probe. |
| `ExistingSocketCheckFailed` | A real socket could not be classified because connect failed with an error other than `ConnectionRefused` or race-related `NotFound`. |
| `StaleSocketCleanupFailed` | Removal of the same confirmed stale socket failed. |
| `BindFailed` | The single listener bind attempt failed. |
| `SocketPermissionFailed` | Applying or verifying mode `0600` on the newly bound socket failed. |
| `AcceptFailed` | Accepting a client connection failed. |
| `ConnectionPreparationFailed` | Preparing an accepted stream for `BufRead` plus `Write` failed. |
| `SocketCleanupFailed` | Explicit cleanup could not inspect or remove the confirmed owned socket safely. |

The implementation may retain an underlying `std::io::Error` as an internal
error source for process diagnostics. It must not expose unnecessary filesystem
details to protocol clients, and these failures are never protocol responses.
`UnixRuntimeError` must not contain `BackendError`, `ProtocolError`, or
Hyprland-specific errors.

A connection served through the handler variant reports failures with
`UnixServeError<E>`, which has three flat variants: `Runtime(UnixRuntimeError)`
for stream preparation, `Transport(TransportError)` for read, write, and flush
failures, and `Handler(E)` for the caller's own handler error. The handler error
does not belong to `UnixRuntimeError`, which remains free of handler, protocol,
and backend errors. `UnixConnectionError`, returned by `serve_unix_connection`,
is unchanged.

## Testability

Path resolution and binding must be separable for deterministic tests:

```text
production environment
    |
    v
resolve and validate XDG_RUNTIME_DIR
    |
    v
bind at validated runtime root
```

Production startup always uses the environment resolver. An internal test
boundary may bind at an explicitly supplied, already validated runtime root.
This is dependency injection for tests, not a public daemon option for choosing
arbitrary production paths.

No `tempfile` dependency is required by this specification. Future tests may
create unique directories under `std::env::temp_dir()` using the process ID and
an atomic counter, with explicit cleanup. Use of a temporary root is permitted
only in tests; production still has no `/tmp` fallback.

Runtime tests use local Unix sockets and a fake backend where protocol behavior
is needed. They require neither a real Hyprland session nor the real
`XDG_RUNTIME_DIR` of the test runner.

## Required future tests

The Unix runtime implementation must test at least:

1. missing `XDG_RUNTIME_DIR`;
2. empty `XDG_RUNTIME_DIR`;
3. relative `XDG_RUNTIME_DIR`;
4. nonexistent `XDG_RUNTIME_DIR` is `InvalidRuntimeDir`;
5. an operational metadata failure is `RuntimeDirUnavailable`;
6. a valid runtime root;
7. rejection of `XDG_RUNTIME_DIR` when its final component is a symlink;
8. rejection of an `XDG_RUNTIME_DIR` mode other than `0700`, without chmod;
9. creation of `clea/` with requested mode `0700` from the creation operation;
10. verified final mode `0700` on `clea/`;
11. an existing real `clea/` directory;
12. rejection of `clea/` as a symlink;
13. rejection of `clea/` as a regular file;
14. bind when the socket path is absent;
15. mode `0600` on the bound socket;
16. `AlreadyRunning` for an existing socket with an active listener;
17. removal and rebind of a real socket that returns `ConnectionRefused`;
18. rejection and preservation of a regular file at the socket path;
19. rejection and preservation of a symlink at the socket path;
20. normal cleanup removes only the socket created by the current owner;
21. cleanup does not remove a path replaced by another object or socket;
22. cleanup is a successful no-op when the socket path is already absent;
23. bind race or failure causes no second destructive cleanup attempt;
24. acceptance of a real `UnixStream` in a test directory;
25. an accepted connection can be passed to `serve_connection`;
26. the protocol works through a real test Unix socket; and
27. no real Hyprland session is required.

Tests involving order-sensitive startup races should use controlled local
fixtures or narrowly scoped test hooks rather than sleeps or external daemon
processes where possible.

## Deliberately out of scope

This specification does not decide or implement:

- systemd integration or socket activation;
- daemonization or `fork`;
- a logging framework;
- a CLI;
- D-Bus;
- Tokio or another async runtime;
- implementation of the multi-client coordinator selected by ADR 0006;
- persistence;
- Hyprland event sockets or `socket2` integration;
- reconciliation or event processing;
- automatic retry loops;
- protocol changes; or
- Window Management policy changes.

## Acceptance criteria

A future Unix runtime adapter conforms to this specification when it:

1. derives the production socket only as
   `$XDG_RUNTIME_DIR/clea/window-management.sock`;
2. fails startup for a missing, empty, relative, inaccessible, or unusable
   runtime root and never falls back to `/tmp`;
3. requires the production runtime root itself to exist as a real, non-symlink
   directory with mode `0700`;
4. never changes permissions on `XDG_RUNTIME_DIR` itself;
5. requests mode `0700` when creating `clea/`, then re-inspects the path and
   verifies its final mode is exactly `0700`;
6. never replaces or removes a non-socket object or symlink at the socket path;
7. never replaces or removes the socket of an active daemon;
8. treats only `ConnectionRefused` on a proven real socket as stale;
9. treats connect `NotFound` as a race and lets one bind attempt decide;
10. performs no retry or second destructive cleanup after bind failure;
11. sets the newly bound socket mode to `0600`, relying on the `0700` parent
    during the bind-to-chmod interval rather than adding `libc` only for umask;
12. records socket identity and removes the path on cleanup only when it still
    identifies the socket created by that listener owner;
13. tolerates an already absent path during cleanup and preserves a replaced
    path;
14. keeps crash recovery in startup stale-socket handling rather than assuming
    `Drop` always runs;
15. keeps Unix runtime errors separate from transport, protocol, backend, and
    Hyprland errors;
16. passes accepted connections to the existing `serve_connection` framing
    implementation;
17. does not interpret protocol methods or duplicate framing;
18. preserves one logical `WindowManagementService` authority rather than
    constructing one per request or connection;
19. leaves daemon coordination outside the adapter, under ADR 0006; and
20. validates the adapter independently before turning `main.rs` into a full
    daemon.
