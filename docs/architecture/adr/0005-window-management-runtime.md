# ADR 0005: Window Management Runtime Architecture

- Status: Accepted
- Date: 2026-09-26

## Context

The Window Management functional contract defines workspace modes, window
policy, transitions, and state invariants independently of implementation and
transport. CLEA now needs a runtime authority that can enforce that contract
without placing policy in the Shell, Settings, CLI clients, QML, or
Hyprland-specific integration code.

The runtime must support long-lived state, independent testing, multiple client
types, and replacement of both presentation clients and compositor backends.

## Decision

### Runtime authority

CLEA Window Management will run as an independent process conceptually named
`clea-windowd`.

`clea-windowd` is the runtime authority for:

- workspace mode;
- window policy;
- transitions between `tiling` and `windows`;
- window-state invariants; and
- future minimize and maximize policy.

Shell, Settings, and future CLI clients request operations and observe state
through the CLEA client protocol. They do not implement these policies or
become runtime authorities for them.

### Implementation language

The initial implementation of `clea-windowd` will use Rust. Rust is selected for
this component because it provides:

- suitability for a long-running service;
- strong typing for policy, state, and protocol boundaries;
- memory safety without requiring a managed runtime;
- good control over concurrency and state ownership;
- low runtime overhead;
- support for testing components without starting the Shell;
- independence from Qt and QML; and
- the ability to maintain and release the daemon independently of the Shell.

Rust is not a universal language requirement for CLEA. This decision applies to
the Window Management daemon and does not constrain the implementation language
of other components.

### Process model

`clea-windowd` will be a per-user daemon and an independent process in the user
session. The Shell is neither responsible for the daemon's lifecycle nor the
owner of its Window Management authority. Restarting or replacing the Shell
must not require that policy ownership move into the Shell.

The concrete startup and supervision mechanism is deferred. This ADR does not
decide whether a systemd user service or another session integration will start
and supervise the daemon.

### Initial client protocol transport

The first client API will use:

- a Unix domain socket;
- a textual JSON protocol;
- unambiguously delimited messages, initially one JSON message per line; and
- an explicit protocol version.

The functional Window Management contract remains independent of this
transport. Unix domain sockets and JSON Lines are the initial transport and
encoding choices, not the architectural identity of the API or its policy.
Client-visible semantics remain defined by documented CLEA contracts.

D-Bus may be added later as an additional adapter or transport without moving
policy into clients or replacing `clea-windowd` as the authority. D-Bus will not
be implemented as part of this decision.

The concrete socket path and complete protocol schema require a later
specification.

### Hyprland backend boundary

Communication between `clea-windowd` and Hyprland will remain behind an
internal backend interface:

```text
Window Management Core
        |
        v
WindowBackend interface
        |
        +-- HyprlandBackend
        +-- Fake/Test Backend
```

`HyprlandBackend` will use Hyprland's native, documented IPC interfaces to:

- observe events;
- query compositor state; and
- request compositor actions.

`hyprctl` is not a permanent architectural dependency or the primary internal
interface between CLEA and Hyprland. Developers may continue to use `hyprctl`
for manual debugging, diagnostics, and development workflows.

The concrete Hyprland sockets, commands, events, message handling, and
reconnection behavior require a later backend specification and implementation.

### Testability

Window Management policy must not depend directly on Hyprland. The core depends
on the internal backend interface so tests can run against a fake backend:

```text
WindowManager
    |
    +-- Real HyprlandBackend
    |
    +-- FakeBackend
```

Without starting Hyprland or the Shell, this boundary must allow tests for:

- different modes in two workspaces;
- isolated workspace transitions;
- newly managed windows;
- windows moving between workspaces;
- fullscreen preservation; and
- idempotent operations.

Protocol and backend adapters may have their own tests, but core policy tests
must not require a real compositor.

### Dependency direction

The conceptual runtime dependency direction is:

```text
Shell / Settings / CLI
        |
        | CLEA client protocol
        v
clea-windowd
        |
        | internal backend interface
        v
HyprlandBackend
        |
        v
Hyprland
```

Dependencies must not be inverted:

- `clea-windowd` must not depend on the Shell;
- QML must not implement Window Management policy; and
- the Window Management core must not depend directly on Hyprland-specific
  commands, sockets, events, or wire formats.

## Deliberately out of scope

This ADR does not decide:

- a specific Rust async runtime such as Tokio or async-std;
- a serialization framework;
- the final crate layout;
- the complete client message schema;
- the Unix socket path;
- a persistence format;
- a systemd unit or another startup mechanism;
- a D-Bus implementation;
- the name of a future CLI;
- minimize or maximize behavior;
- Windows Mode geometry;
- exception policy for special window types; or
- the concrete Hyprland reconnection mechanism.

These choices require separate specifications or decisions. No implementation
is introduced by this ADR.

## Consequences

### Advantages

- The Shell remains replaceable and independent of Window Management authority.
- Policy can be tested without starting the Shell or Hyprland.
- Hyprland-specific details remain isolated in a replaceable backend.
- Shell, Settings, and CLI clients can reuse the same policy authority and
  observe consistent state.
- The daemon can survive Shell restarts and continue to own runtime state.
- Rust provides a low-overhead, strongly typed foundation for a long-running
  service.
- The backend and client protocol boundaries provide a suitable base for future
  compositor, transport, and desktop integrations.

### Costs and responsibilities

- The user session contains one additional process.
- CLEA must define and maintain a protocol between clients and the daemon.
- The runtime and clients must handle disconnections, reconnections, daemon
  failures, and compositor failures.
- The client API requires explicit versioning and compatibility management.
- The daemon lifecycle needs a future startup and supervision decision.
- Textual JSON messages have parsing and validation costs, even though they
  simplify inspection and early integration.
- Backend abstraction adds an interface that must remain aligned with both core
  policy needs and Hyprland capabilities.
