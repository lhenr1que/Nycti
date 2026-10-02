# Hyprland Backend Specification

## Status and scope

This document specifies the first Hyprland-specific adapter work for Nycti
Window Management. It refines the compositor-independent
`WindowBackend` contract without changing that contract, the Window Management
functional contract, or their ownership boundaries.

The first concrete integration will be developed and initially validated only
against:

```text
Hyprland 0.56.2
```

Compatibility with earlier or later Hyprland versions is not claimed. Support
for another version requires separate validation of its IPC requests, response
schemas, identities, actions, and events.

Hyprland-specific socket paths, request strings, response fields, identities,
and event data are private implementation details of this adapter. The Window
Management core remains compositor-independent and must not depend on them.

## Relationship to the backend boundary

The current Rust `WindowBackend` trait requires all of these capabilities:

- list workspaces;
- list windows;
- ensure a window is tiled;
- ensure a window is floating; and
- focus a window.

The first implementation stage specified here was read-only and therefore did
not satisfy the complete trait; `HyprlandBackend` now implements it fully, with
the actions specified in the
[Hyprland actions specification](hyprland-actions-spec.md). It must not
implement action methods by
returning false success, silently doing nothing, or substituting toggle
semantics.

Until actions are separately specified, the implementation may introduce a
private Hyprland parser or snapshot source, conceptually named
`HyprlandSnapshotSource`, without implementing `WindowBackend`. The concrete
`HyprlandBackend` should implement the complete trait only after translations
for declarative placement and focus actions have been specified and tested.

This staging does not weaken the backend contract. It prevents a partial
adapter from claiming capabilities it does not have.

## Native IPC transport

### Runtime dependency

`HyprlandBackend` will communicate directly with Hyprland's documented Unix
IPC. It will not spawn or otherwise depend on `hyprctl` at runtime.

`hyprctl` remains appropriate only for:

- development;
- debugging; and
- manual diagnostics.

### Request socket path

The request socket path is:

```text
$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock
```

The adapter must derive this path at runtime from the `XDG_RUNTIME_DIR` and
`HYPRLAND_INSTANCE_SIGNATURE` environment variables. It must not hardcode a
user ID, runtime directory, instance signature, or complete socket path.

A missing or unusable environment value, a missing socket, or a connection
failure prevents communication with the compositor and maps conceptually to
`BackendError::CompositorUnavailable`. This document does not change the error
enum or prescribe the final internal error representation.

### Synchronous request lifecycle

The Hyprland request socket is synchronous. For each request, the first
implementation will:

1. open a Unix socket connection;
2. send exactly one request;
3. read that request's response;
4. close the connection promptly.

The first implementation will not keep a persistent request-socket connection
and will not batch multiple requests over one connection. Timeout, retry, and
reconnection policies are not selected by this specification.

### Read requests

The read-only stage uses these request payloads:

```text
j/workspaces
j/clients
j/activewindow
```

Their roles are:

- `j/workspaces`: source of currently present workspace observations;
- `j/clients`: primary source of window observations and workspace membership;
- `j/activewindow`: authoritative source used to identify the focused window.

The implementation must keep request transport separate from JSON parsing so
recorded response fixtures can exercise parsing and normalization without a
running Hyprland instance.

## Structured JSON parsing

The implementation is expected to use structured JSON parsing. `serde` and
`serde_json` are acceptable planned dependencies for the Hyprland adapter, but
this specification does not add them.

Wire-format structs, if used, are private to the Hyprland adapter. They should
contain only fields needed for validation, identity correlation, and the first
normalization. Unknown response fields may be ignored. Fields required by this
specification must not be silently defaulted when absent or incompatible.

The initial required data is:

| Response | Required data |
|---|---|
| `j/workspaces` | top-level array; each entry's `id` |
| `j/clients` | top-level array; each entry's `stableId`, `address`, `workspace.id`, `floating`, and `fullscreen` |
| `j/activewindow` | active window `stableId` and `address` when an active window exists |

The exact representation returned by `j/activewindow` when no window is active
has not yet been observed or confirmed for the target version. Its parsing and
normalization remain an explicit case to specify before implementation; the
adapter must not invent a sentinel representation.

## Backend-owned identities

### General rule

Hyprland identities must not escape through the compositor-independent API.
The existing public representations of `WindowId` and `WorkspaceId` remain
unchanged by this specification.

The adapter may allocate backend-local Nycti IDs and retain private maps between
those IDs and Hyprland identities. The core can compare and return opaque Nycti
IDs but cannot construct them from Hyprland data or extract Hyprland data from
them.

### Window identity

`WindowId` must not be a direct encoding or alias of the Hyprland window
address. The conceptual private association is:

```text
Nycti WindowId
    -> HyprlandWindowIdentity {
           stable_id,
           address,
       }
```

For Hyprland 0.56.2:

- `stableId` is a hexadecimal string without a `0x` prefix in JSON;
- its corresponding local Hyprland value is an unsigned 64-bit stable ID;
- `address` is a `0x`-prefixed window-address string;
- both fields are present in the observed `j/clients` and `j/activewindow`
  schemas.

`stableId` is the primary identity for correlating entries across query
snapshots. When a previously known `stableId` remains present, it retains its
backend-local Nycti `WindowId`. Different simultaneously observed `stableId`
values must receive different Nycti `WindowId` values.

`address` is retained as a secondary handle because:

- it is present in query responses;
- Hyprland supports address-based window selectors; and
- relevant public socket2 events commonly identify windows by
  `WINDOWADDRESS`.

The adapter must maintain enough private association to resolve both
`stableId -> WindowId` and `address -> WindowId`. A contradictory snapshot,
such as duplicate primary identities for distinct client entries or one active
identity ambiguously matching multiple clients, is not a successful normalized
snapshot.

No persistence or cross-session meaning is assigned to this mapping. In
particular, the adapter must not assume that a Hyprland `stableId` or a Nycti
`WindowId` survives:

- closing and reopening a window;
- restarting Hyprland; or
- starting a new user session.

The exact identity-retention rule after a window disappears from a snapshot is
not fixed here. It must be decided before event handling and stale-identity
behavior are implemented.

### Workspace identity

Hyprland `workspace.id` is the primary native workspace identity observed for
the target version. The conceptual private association is:

```text
Nycti WorkspaceId
    -> Hyprland workspace.id
```

The adapter allocates or looks up an opaque Nycti `WorkspaceId` for a native
workspace ID. It must not use `workspace.name` as the primary identity and must
not expose the native numeric ID to the core.

The adapter may encounter a workspace ID through either `j/workspaces` or a
client's `workspace.id`; the same native ID must map to the same Nycti identity
within the applicable backend-local lifetime.

No complete semantics for special-workspace IDs are selected. In particular,
the implementation must not rely on an unverified sign, range, name prefix, or
lifetime rule for special workspaces.

## Workspace normalization

`j/workspaces` returns a JSON array. For each valid entry in a successful
response, the read-only source produces one `WorkspaceObservation` with:

- the opaque Nycti `WorkspaceId` associated with Hyprland `workspace.id`; and
- `present = true`.

Only entries in the current response are returned. Retaining observations for
workspaces that disappeared is outside the first implementation. Workspace
mode is not read from Hyprland because default, explicit, and effective modes
are Nycti-owned policy.

Workspace name, monitor, window count, last window, persistence, fullscreen
summary, and layout name are not part of the first normalized observation.

## Window normalization

`j/clients` is the primary source of the window snapshot. Every successfully
normalized client entry produces one `WindowObservation`.

### Placement

Placement is normalized as follows:

```text
Hyprland floating == true
    -> WindowPlacement::Floating

Hyprland floating == false
    -> WindowPlacement::Tiled
```

### Workspace membership

The client's `workspace.id` is resolved through the backend's private workspace
identity mapping and becomes the opaque `WorkspaceId` in
`WindowObservation`. Workspace name is not used as identity.

### Excluded metadata

The first normalized API does not use or expose:

- title;
- class or initial class;
- PID;
- geometry or size;
- monitor;
- tags; or
- other application metadata.

Such fields may appear in private wire structs only if technically necessary
for safe parsing. Their mere presence in Hyprland responses does not make them
part of Nycti's normalized model.

## Fullscreen normalization

Hyprland's `fullscreen` field is numeric and must not be deserialized or treated
as a boolean. The first normalization is:

| Hyprland `fullscreen` | `WindowObservation::is_fullscreen()` |
|---:|---|
| `0` | `false` |
| `1` | `false` |
| `2` | `true` |
| `3` | `true`, if received |

Value `1` represents maximized state in the locally inspected Hyprland 0.56.2
enum and therefore must not be confused with fullscreen. This specification
does not add maximized state to the Nycti backend model.

The normalization uses Hyprland's internal `fullscreen` field.
`fullscreenClient` does not determine `WindowObservation::is_fullscreen()` in
this first version.

The local 0.56.2 enum exposed values `0`, `1`, and `2`; accepting `3` as
fullscreen is an explicit normalization rule, not a claim that it was observed
in the audited session. Any real target-version data that contradicts this
table must be reported and resolved before implementation proceeds. Values
outside the specified set have no normalization in this document.

## Focus normalization

`j/activewindow` is the authoritative focus source. `focusHistoryID` from
`j/clients` is not authoritative and is not used to assign focus.

When `j/activewindow` describes an active window, correlation proceeds in this
order:

1. match its `stableId` against the client snapshot;
2. use `address` only as a secondary correlation when necessary.

For a successful correlated snapshot:

- exactly the matching `WindowObservation` has `focused = true`; and
- every other `WindowObservation` has `focused = false`.

If active-window and client responses cannot be correlated because the window
disappeared between synchronous requests, this specification does not invent a
retry. The final handling of that race remains to be specified. Likewise, the
no-active-window response must be observed or documented before its behavior is
implemented.

The three requests are individually synchronous but are not assumed to form an
atomic compositor snapshot.

## Future action support

Hyprland 0.56.2 has locally evidenced capabilities for:

- explicitly setting floating state;
- explicitly unsetting floating state, resulting in tiled placement;
- focusing a specific window;
- selecting a window with `stableid:`; and
- selecting a window with `address:`.

The eventual backend translation must preserve Nycti's declarative semantics:

```text
ensure_floating(window) -> ensure the final state is floating
ensure_tiled(window)    -> ensure the final state is tiled
```

It must never depend semantically on a toggle operation. Repeating either
request must not invert an already-correct placement.

This document does not fix the exact Lua dispatcher expression or selector used
for those actions because the installed help and generated stubs do not fully
specify their public argument syntax. Placement and focus actions will be
specified and tested separately before the concrete adapter implements
`WindowBackend`.

No real placement or focus command is part of the read-only stage.

## Future event support

The future event source is:

```text
$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock
```

Relevant public Hyprland events frequently identify a window with
`WINDOWADDRESS`, including events equivalent to:

- `activewindowv2`;
- `openwindow`;
- `closewindow`;
- `movewindow` or `movewindowv2`; and
- `changefloatingmode`.

Consequently, retaining the private `address <-> WindowId` correlation is a
requirement even though `stableId` is primary for query-snapshot identity.

This specification does not define or select:

- an event loop;
- a thread model;
- Tokio or another async runtime;
- channels or callbacks;
- buffering;
- reconnect behavior;
- event ordering guarantees; or
- reconciliation after dropped or ambiguous events.

The read-only stage must not connect to socket2.

## Error mapping

The initial conceptual mapping is:

| Condition | Existing backend category |
|---|---|
| Required environment value unavailable or unusable | `CompositorUnavailable` |
| Request socket missing or connection fails | `CompositorUnavailable` |
| Request cannot be written or response cannot be read | `CompositorUnavailable` |
| Response is not valid JSON or is incomplete | `ObservedStateUnavailable` |
| Valid JSON has an incompatible required shape or required field | `InconsistentObservedState` |
| Required identities are duplicate or contradictory | `InconsistentObservedState` |

A window disappearing between `j/clients` and `j/activewindow` is a known race
whose final treatment remains open. No retry or fallback policy is introduced
here.

This document does not alter `BackendError`, add error variants, or define
transport-specific errors visible to the core.

## Runtime and dependency constraints

The request path and parsing stage may remain synchronous. This specification
does not select Tokio, async I/O, or any async runtime.

The first implementation may add `serde` and `serde_json` when implementation
work begins. No dependency is added by this document, and adding those crates
will require normal implementation review.

The transport, wire structs, native identities, and identity maps belong in the
Hyprland adapter. They must not move into the core, fake backend, or
client-protocol model.

## Acceptance criteria for the future read-only implementation

The read-only implementation is acceptable when tests demonstrate that:

1. the request socket path is derived from `XDG_RUNTIME_DIR` and
   `HYPRLAND_INSTANCE_SIGNATURE`, not hardcoded;
2. a valid `j/workspaces` response produces `WorkspaceObservation` values;
3. a valid `j/clients` response produces `WindowObservation` values;
4. `floating = true` normalizes to `WindowPlacement::Floating` and
   `floating = false` normalizes to `WindowPlacement::Tiled`;
5. `fullscreen = 0` does not report fullscreen;
6. `fullscreen = 1` does not report fullscreen and is not added to the model as
   maximized;
7. `fullscreen = 2` reports fullscreen;
8. `fullscreen = 3`, if received, reports fullscreen;
9. window membership is derived from `workspace.id`, not workspace name;
10. `j/activewindow` determines which observation is focused;
11. neither `stableId` nor `address` escapes to the core-facing observation
    API;
12. two simultaneously observed windows with different `stableId` values
    receive distinct Nycti `WindowId` values;
13. `stableId` is the primary identity used to correlate query snapshots;
14. `address` remains privately available for future event correlation;
15. fixture-based read-only tests require no running Hyprland when JSON
    responses are supplied; and
16. later manual or integration tests may use a real Hyprland instance without
    moving policy or Hyprland wire details into the core.

Tests must also demonstrate that the read-only component does not claim action
success and does not require an implementation of the complete
`WindowBackend` trait before actions are specified.

## Deliberately out of scope

The following are outside this specification:

- real placement commands;
- real focus commands;
- a complete `WindowBackend` implementation before actions are specified;
- socket2 connections;
- event loops;
- async I/O;
- Tokio;
- reconnection;
- retries;
- timeout policy;
- complete special-workspace semantics;
- maximize or minimize modeling and actions;
- geometry;
- monitor modeling;
- exception classification;
- persistence; and
- compatibility with Hyprland versions other than 0.56.2.

## Remaining specification questions

Before the read-only stage is implemented, the following must be resolved if
encountered by its tests or target environment:

1. the exact `j/activewindow` response when no window is active;
2. the result of an active-window/client race, without assuming retries;
3. handling of a `fullscreen` value outside `0` through `3`;
4. the backend-local retention rule after a window or workspace disappears;
5. any special-workspace ID needed by the first target environment.

Before the complete `HyprlandBackend` implements `WindowBackend`, separate
action specifications must resolve:

1. exact Lua dispatcher requests for set-floating, unset-floating, and focus;
2. exact use and validation of `stableid:` and `address:` selectors;
3. idempotence and postcondition verification;
4. stale-window behavior between lookup and action; and
5. mapping of action failures to the existing backend errors.
