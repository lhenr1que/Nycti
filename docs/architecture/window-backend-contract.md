# Window Backend Contract

## Status and scope

This document defines the first conceptual contract between the Nycti Window
Management core and compositor backends. It is compositor-independent and is
intended to guide a future Rust trait, a Hyprland backend, and a fake backend for
tests without defining their concrete APIs yet.

The boundary is:

```text
Window Management Core
        |
        v
WindowBackend contract
        |
        v
compositor backend
```

The core owns policy. A backend observes compositor state and applies requested
outcomes. A backend does not decide workspace modes, placement policy, or
fullscreen preservation policy.

## Dependency direction

Implementations follow this dependency direction:

```text
core
 |
 v
backend contract
 |
 +-- HyprlandBackend
 +-- FakeBackend
```

The core depends only on normalized Nycti concepts. It does not know about
Hyprland sockets, `hyprctl`, dispatchers, event names, compositor addresses, or
wire formats. Those details stop at a concrete backend.

## Opaque identities

### `WorkspaceId`

`WorkspaceId` conceptually identifies a workspace across observations and
workspace references at the backend boundary.

Core consumers must not construct a `WorkspaceId` from a workspace name,
numeric index, Hyprland address, or another compositor-specific value. The
concrete backend maps its internal workspace identity to an opaque Nycti
identity.

### `WindowId`

`WindowId` conceptually identifies a window across observations and action
requests at the backend boundary.

Core consumers must not construct a `WindowId` from a Hyprland address, PID,
window title, application class, or another item of compositor or application
metadata. The concrete backend maps its internal window identity to an opaque
Nycti identity.

The concrete representation and lifetime rules for both identity types are
outside this contract. This document does not select strings, integers, UUIDs,
or Rust types for them.

## Observed state

Backend observations report facts from the compositor. They do not contain Nycti
policy decisions.

### Workspace observation

For every workspace known at the boundary, the core must be able to observe:

- its opaque `WorkspaceId`; and
- whether the workspace is currently present.

Workspace mode is deliberately absent. Default, explicit, and effective
workspace modes are Nycti-owned policy and are not compositor state.

This contract does not define how long a backend retains the identity or
observation of a workspace that is no longer present.

### Window observation

For every known window, the core must be able to observe:

- its opaque `WindowId`;
- the opaque `WorkspaceId` of its containing workspace;
- its normalized `WindowPlacement`, either `Tiled` or `Floating`;
- whether it is fullscreen; and
- whether it is focused.

The first backend contract does not require maximized state. Maximized remains a
future capability, consistent with the functional contract's conditional
reporting of that state. Minimized state is also outside this version.

Geometry, title, application class, PID, monitor, opacity, decorations, tags,
and application metadata are not part of the first observation model. Such
fields may be added only when a documented policy has a concrete need for them.

## Read capabilities

The backend must conceptually let the core obtain:

1. the workspaces currently known to the backend and their presence state; and
2. the windows currently known to the backend with all window observations
   defined above.

These capabilities are sufficient for the core to determine each window's
workspace membership, observed placement, fullscreen state, and focus state.
Focus may be represented directly on each listed window; a separate focus query
is not required by this contract.

The future API should avoid redundant read operations when one coherent listing
can provide the required information. This statement does not define atomicity,
snapshot isolation, method names, or return types.

## Action capabilities

The backend must conceptually accept requests to:

- ensure that a window is tiled;
- ensure that a window is floating; and
- focus a window.

Each request identifies its target with an opaque `WindowId`. This first
contract does not include actions for maximize, minimize, geometry changes,
moving a window between workspaces, or entering or leaving fullscreen.

### Declarative placement semantics

Placement requests describe desired outcomes:

```text
make tiled(window)    = ensure the window is tiled
make floating(window) = ensure the window is floating
```

They never mean "toggle tiled" or "toggle floating." A concrete backend is
responsible for translating a declarative outcome into an appropriate
compositor operation while hiding compositor-specific state and commands from
the core.

The policy planner normally avoids redundant requests, but backend semantics do
not depend on that optimization. Repeating a placement request for a window
already in the requested state must not invert its placement.

Focus is likewise an outcome request for the identified window, not a
compositor-specific command exposed to the core.

## Fullscreen preservation

The backend reports fullscreen state but exposes no action to enter, leave, or
toggle fullscreen in this first contract.

For ordinary workspace mode changes, the core preserves fullscreen by producing
`WindowPlacementAction::Keep`. A backend is therefore not asked to change the
placement of that fullscreen window as part of the mode change. The boundary
must not provide a shortcut that encourages leaving fullscreen, changing
placement, and restoring fullscreen.

The placement to apply after fullscreen later ends remains outside this
contract, as established by the functional contract.

## Relationship to the placement planner

The existing planner consumes normalized observations and produces a
compositor-independent policy result:

```text
ObservedWindowState
        |
        v
plan_window_placement()
        |
        v
WindowPlacementAction
        |
        +-- Keep --------------------> no backend action
        +-- MakeTiled --------------> ensure tiled via WindowBackend
        +-- MakeFloating -----------> ensure floating via WindowBackend
```

`WindowPlacementAction` remains a core policy result. It must not contain
Hyprland commands, dispatcher strings, addresses, or wire data. Translation
from an action to a backend request occurs at the backend contract boundary.

## Errors

The boundary must be able to report at least these conceptual failure
categories:

- unknown or stale `WindowId`;
- unknown or stale `WorkspaceId`;
- compositor unavailable;
- requested action failed; and
- inconsistent or unavailable observed state.

This document does not define a Rust error enum, error library, numeric codes,
retry policy, timeouts, reconnection behavior, or partial-failure strategy.
Those decisions require the concrete API and runtime requirements.

## Future event capability

A compositor backend will eventually need to provide relevant state changes to
the core so that Nycti can react to new windows, changed state, focus changes,
and workspace membership changes.

This contract does not define an event API. Event format, streaming model,
callbacks, channels, async runtime, subscriptions, buffering, and event-loop
behavior remain outside scope. The initial trait design must not invent these
details before their requirements are specified.

## Fake backend

The contract must be implementable by a `FakeBackend` that runs without
Hyprland. A future fake implementation must be able to:

- provide workspace observations;
- provide window observations and workspace membership;
- record declarative ensure-tiled requests;
- record declarative ensure-floating requests; and
- record focus requests.

This capability will support integration tests between core policy and the
backend boundary. This document does not implement the fake or prescribe its
storage, setup API, or assertion API.

## Deliberately out of scope

This contract does not decide:

- the concrete language-level representation of the trait;
- final Rust method names or signatures;
- synchronous versus asynchronous methods;
- ownership, borrowing, or lifetime design;
- concrete `Result` and error enum types;
- Tokio or another runtime;
- channels, callbacks, or event subscriptions;
- Hyprland IPC commands, events, socket paths, or wire formats;
- reconnection behavior;
- timeout or retry policy;
- partial-failure handling;
- monitor modeling;
- geometry;
- maximize or minimize state and actions;
- exception classification;
- persistence; or
- moving windows between workspaces.

Windows moved by users, applications, or the compositor must eventually be
observable through refreshed state or the future event capability, but this
first contract does not provide an action for initiating such a move.

## Acceptance criteria

The first backend contract is satisfied when tests can demonstrate that:

1. a backend supplies two workspace observations without exposing Hyprland
   workspace identifiers to the core;
2. a backend supplies window observations with membership represented by opaque
   `WorkspaceId` values;
3. the core can observe each window's tiled or floating placement, fullscreen
   state, and focus state;
4. an ensure-tiled request is declarative and does not toggle a window that is
   already tiled;
5. an ensure-floating request is declarative and does not toggle a window that
   is already floating;
6. a focus request targets an opaque `WindowId` rather than compositor-specific
   window data;
7. preserving a fullscreen window requires no enter, leave, or toggle
   fullscreen backend operation;
8. a `FakeBackend` can satisfy all first-version reads and actions without
   starting Hyprland;
9. the core and its tests require no Hyprland commands, addresses, event names,
   sockets, or wire formats; and
10. `WindowPlacementAction::Keep` produces no backend action.
