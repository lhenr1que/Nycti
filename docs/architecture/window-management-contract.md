# Window Management Functional Contract

## Status and scope

This document defines the first functional contract for CLEA Window Management.
It specifies observable behavior and ownership boundaries without selecting an
implementation language, IPC transport, process model, persistence format, or
Hyprland integration mechanism.

The first version covers:

- a window-management mode for each workspace;
- a default workspace mode and workspace-specific choices;
- normalized window and workspace state;
- workspace mode query and change operations;
- window listing and focus operations;
- transitions between tiling-oriented and free-window modes; and
- preservation of fullscreen state.

Maximize and minimize operations are outside this version.

## Workspace modes

Each workspace has exactly one effective mode at runtime:

### `tiling`

Ordinary, policy-managed windows in the workspace are expected to participate
in tiled layout. The mode describes CLEA policy; it is not a direct exposure of
a compositor-specific setting.

### `windows`

Ordinary, policy-managed windows in the workspace are expected to be floating
and independently positionable. The mode name is part of the CLEA contract and
does not imply a specific compositor command, geometry policy, or
implementation.

The mode belongs to the workspace, not to individual windows. A window has an
observed placement state and follows the effective policy of the workspace in
which it is managed; it does not carry a `tiling` or `windows` mode of its own.

## Mode resolution

The three mode concepts have distinct meanings:

### Default workspace mode

The **default workspace mode** is one of `tiling` or `windows`. It provides the
fallback policy for every workspace without a workspace-specific choice.

How the default is selected, configured, or persisted between sessions is not
defined in this version.

### Explicit workspace mode

An **explicit workspace mode** is an optional workspace-specific choice of
`tiling` or `windows`. When present, it overrides the default for that workspace
only. Changing one workspace's explicit mode does not change the default or any
other workspace's explicit mode.

### Effective workspace mode

The **effective workspace mode** is the policy currently applied to ordinary
windows in a workspace:

```text
effective workspace mode = explicit workspace mode, when present
                           default workspace mode, otherwise
```

Two workspaces may therefore have different effective modes at the same time.
Persistence of explicit choices between sessions remains outside this version.

## Relevant window states

The contract uses normalized, compositor-independent terms:

- **tiled**: the window participates in tiled layout;
- **floating**: the window is outside tiled layout and may be positioned
  independently;
- **fullscreen**: the window occupies its fullscreen presentation and is
  protected from workspace mode transitions;
- **maximized**: the window occupies the applicable usable area without being
  fullscreen; the state is recognized by the model, but changing it and its
  interaction with workspace mode transitions are outside this version;
- **minimized**: reserved for a future version and not supported by the initial
  operations or transition rules.

For the initial contract, `tiled` and `floating` are mutually exclusive
placement states. `fullscreen` is an independent state that takes precedence
over the visible placement behavior. This contract does not require a workspace
mode transition to alter the underlying placement state of a fullscreen window.

The representation and lifecycle of `maximized` and future `minimized` state are
not defined here. Clients must not infer maximize or minimize capabilities from
their presence in this state vocabulary.

## Placement and visibility

Workspace mode controls window placement policy, not window visibility. Tiled
and floating describe placement; minimized describes a separate, future
visibility state. Changing or applying a workspace mode must not implicitly
restore minimized windows.

When a new ordinary window is created in a workspace, only that new window
adopts the workspace's effective placement policy. Creating or opening it must
not restore other minimized windows or otherwise change their minimized state.

A minimized window may retain its underlying placement while it is not visible.
For example, a minimized window in `windows` mode may remain logically floating.
Restoring a minimized window is a separate explicit operation and is not a side
effect of applying workspace placement policy.

These rules preserve the architectural separation between placement and
visibility without defining minimized-state representation, compositor
mechanisms, taskbar behavior, or restore operations in this version.

### Conceptual example

Consider workspace 1 in `windows` mode:

- Browser: minimized, with underlying floating placement.
- Terminal: minimized, with underlying floating placement.

When a new window opens:

- the new window is visible and floating;
- Browser remains minimized; and
- Terminal remains minimized.

This example illustrates state separation only. It does not establish a user
interface, taskbar, or compositor implementation requirement.

## Workspace and window identity

Each workspace has an opaque identity that clients can use with workspace mode
operations. Each listed window has an opaque identity that clients can return to
the focus operation and an associated opaque workspace identity.

Clients must not construct these identities from names, indices, titles,
application names, process identifiers, or compositor-specific addresses. Their
encoding, lifetime, and concrete mapping to Hyprland are implementation details
of a future API contract.

## Initial operations

Operation names below describe behavior, not function names or wire messages.

### Query workspace mode

Accepts a workspace identity and returns enough information to distinguish:

- the default workspace mode;
- whether the workspace has an explicit mode and, if so, its value; and
- the resulting effective workspace mode.

The operation does not change workspace or window state.

### Change workspace mode

Conceptually accepts:

- a workspace identity; and
- a target mode of `tiling` or `windows`.

The operation establishes or replaces the explicit mode for that workspace and
applies the corresponding transition policy only to ordinary, policy-managed
windows in that workspace. It does not alter the default mode or another
workspace's mode or windows.

Requesting the already-explicit mode is idempotent. Requesting a mode equal to
the workspace's inherited effective mode may establish an explicit choice, but
must not toggle window state merely to recreate the same effective result.

A successful result means the workspace has the requested explicit and
effective mode and the transition rules have been applied to its in-scope
windows that can be managed. The representation of errors, partial failure,
retries, and concurrency is not defined in this document.

No operation for clearing an explicit mode or changing the default mode is
defined in this first version.

### List windows

Returns the currently known windows with, at minimum:

- opaque window identity;
- opaque identity of the workspace containing the window;
- normalized `tiled` or `floating` placement when applicable;
- fullscreen state;
- maximized state when it can be determined; and
- whether the window is currently focused.

The operation reports state but does not change it. Raw Hyprland workspace or
window data is not part of the client-facing contract.

### Focus window

Accepts an opaque window identity obtained from the window list and requests
focus for that window. On success, the requested window is focused. Focusing a
window does not by itself change any workspace mode, workspace membership,
placement, fullscreen, or maximized state.

Behavior for an unknown, expired, or non-focusable identity, including the
shape of the resulting error, remains to be defined by a future API contract.

## Workspace policy application

The effective mode of a workspace applies to:

- ordinary windows already managed in that workspace when its mode changes;
- ordinary windows that subsequently become managed in that workspace; and
- ordinary windows moved into that workspace from another workspace.

Moving a window does not move or copy a mode with it. Once moved, an ordinary
window follows the destination workspace's effective mode. The source and
destination workspaces retain their own modes.

Rules for maximized windows, fullscreen windows moved between workspaces, and
candidate exception types are deliberately outside the first version except for
the fullscreen invariant during a workspace mode change.

## Transition rules

The normative transition rules apply only to ordinary, policy-managed windows
in the workspace whose mode is being changed and whose placement is `tiled` or
`floating`.

### `tiling` to `windows`

When changing a workspace from effective `tiling` mode to explicit `windows`
mode:

1. CLEA identifies the in-scope windows belonging to that workspace.
2. Every fullscreen window in that workspace remains fullscreen and is not
   toggled out of fullscreen as an intermediate step.
3. Each in-scope, non-fullscreen tiled window in that workspace becomes
   floating.
4. Windows already floating are not toggled solely to reapply the same state.
5. No window in another workspace is reorganized by the transition.
6. The target workspace's explicit and effective mode becomes `windows` when
   the transition succeeds.

This version requires the resulting floating state, but does not define window
size, position, stacking order, workspace movement, or geometry restoration.

### `windows` to `tiling`

When changing a workspace from effective `windows` mode to explicit `tiling`
mode:

1. CLEA identifies the in-scope windows belonging to that workspace.
2. Every fullscreen window in that workspace remains fullscreen and is not
   toggled out of fullscreen as an intermediate step.
3. Each in-scope, non-fullscreen floating window in that workspace becomes
   tiled.
4. Windows already tiled are not toggled solely to reapply the same state.
5. No window in another workspace is reorganized by the transition.
6. The target workspace's explicit and effective mode becomes `tiling` when the
   transition succeeds.

This version requires the resulting tiled state, but does not define layout,
split direction, window order, workspace movement, or restoration of previous
tiling positions.

## Fullscreen invariant

A workspace mode change must never cause a fullscreen window to lose
fullscreen. This applies in both transition directions and includes transient
behavior: an implementation must not exit and then re-enter fullscreen as part
of applying a workspace mode transition.

Fullscreen windows in other workspaces are also unaffected because a workspace
mode change cannot reorganize windows outside the target workspace.

The placement to apply after a user or application later exits fullscreen is
not decided by this version. Whatever future behavior is chosen must not weaken
the invariant during the workspace mode transition itself.

## Candidates for future exceptions

The following window types may require policy different from ordinary windows:

- dialogs;
- popups;
- picture-in-picture windows;
- launchers;
- games; and
- other windows with application-specific, role-specific, or user-defined
  rules.

This list identifies candidates, not current classifications or exemptions. A
future contract must define how each type is recognized, whether it participates
in each workspace mode, and which state must be preserved. Clients and backends
must not assume behavior for these categories from this document.

## State and policy ownership

### State supplied by Hyprland

Through the dedicated backend boundary, Hyprland supplies observations needed
to describe current compositor state, including:

- the set of workspaces and windows currently known to the compositor;
- enough backend-local identity to correlate observations and actions;
- each window's current workspace membership;
- current focus;
- observed tiled, floating, and fullscreen window state; and
- available metadata that may support future classification rules.

These are observations, not the client-facing data format. Hyprland-specific
workspace identifiers, names, addresses, commands, events, and wire formats stop
at the backend boundary. Whether Hyprland directly supplies a maximized or
minimized concept is not assumed by this contract.

### State and policy maintained by CLEA

CLEA owns:

- the default workspace mode;
- the optional explicit mode associated with each workspace;
- resolution of each workspace's effective mode;
- the meaning of `tiling` and `windows` in this contract;
- selection of windows that are in scope for a workspace transition;
- application of workspace policy through the backend boundary;
- normalization of compositor observations into client-facing workspace and
  window state;
- enforcement of workspace isolation and the fullscreen invariant; and
- future exception rules and any CLEA-specific maximize or minimize semantics.

Clients may query this state and request operations, but do not own or apply the
policy themselves.

### Behavior not yet decided

This version intentionally does not decide:

- implementation language or process topology;
- IPC mechanism, serialization, function names, or wire schema;
- direct Hyprland integration technique;
- concrete mapping or identification of Hyprland workspaces;
- selection or runtime modification of the default workspace mode;
- persistence format or persistence across sessions;
- clearing an explicit workspace mode to resume use of the default;
- lifecycle of explicit choices when workspaces disappear or reappear;
- atomicity, ordering, timeout, retry, concurrency, or partial-failure behavior;
- window geometry, placement, stacking, animation, or workspace movement;
- graphical interface;
- exact workspace and window identity encoding and lifetime;
- recognition and treatment of exception candidates;
- maximize behavior or the treatment of maximized windows during transitions;
- minimize representation, mechanisms, and behavior beyond the placement and
  visibility separation defined above;
- placement after a fullscreen window later leaves fullscreen;
- treatment of a fullscreen or maximized window moved between workspaces; and
- policy for windows created or moved while a transition is in progress.

## Acceptance criteria for the first version

The contract is satisfied when the following behaviors can be tested through a
backend-independent test boundary:

1. **Default fallback:** given a default mode and a workspace without an
   explicit mode, querying that workspace reports no explicit mode and reports
   the default as its effective mode.
2. **Explicit override:** given an explicit workspace mode different from the
   default, querying the workspace reports the default, the explicit choice,
   and the explicit choice as the effective mode.
3. **Different simultaneous modes:** two workspaces can simultaneously report
   different effective modes, with ordinary windows tiled in a `tiling`
   workspace and floating in a `windows` workspace.
4. **Isolated tiling-to-windows transition:** changing one workspace from
   `tiling` to `windows` makes its in-scope non-fullscreen windows floating while
   preserving every other workspace's modes and window states.
5. **Isolated windows-to-tiling transition:** changing one workspace from
   `windows` to `tiling` makes its in-scope non-fullscreen windows tiled while
   preserving every other workspace's modes and window states.
6. **Fullscreen preservation:** a fullscreen window remains continuously
   fullscreen throughout a mode transition of its workspace in either
   direction, while fullscreen windows in other workspaces are untouched.
7. **Mixed target workspace:** in a target workspace containing fullscreen and
   ordinary windows, only its in-scope non-fullscreen windows change placement
   according to the target mode.
8. **Idempotent mode request:** requesting the already-explicit mode leaves all
   tested window states unchanged. Establishing an explicit mode equal to an
   inherited effective mode also leaves window states unchanged.
9. **New ordinary window:** a newly managed ordinary window adopts the effective
   mode of the workspace in which it is managed without changing that or any
   other workspace's mode.
10. **Window moved between workspaces:** an ordinary non-fullscreen window moved
    between workspaces with different effective modes adopts the destination
    workspace's placement policy; both workspace modes and unrelated windows
    remain unchanged.
11. **Mode belongs to workspace:** moving an ordinary window does not transfer
    the source workspace's mode to the destination or retain a per-window copy
    of that mode.
12. **Window listing:** listing windows returns one opaque window identity and
    an opaque containing-workspace identity for each known window, and accurately
    reports focus, fullscreen, and applicable normalized placement state without
    mutating any state.
13. **Focus:** focusing a listed, focusable window makes it focused without
    changing workspace modes, workspace membership, placement, fullscreen, or
    maximized state.
14. **Backend isolation:** the client-facing contract and its tests do not
    expose or require Hyprland-specific workspace identifiers, commands,
    addresses, or wire formats.
15. **Windows Mode preserves minimized state:** applying `windows` mode does not
    restore minimized windows.
16. **Tiling Mode preserves minimized state:** applying `tiling` mode does not
    restore minimized windows.
17. **New-window placement is isolated:** opening a new ordinary window applies
    the workspace's effective placement policy only to that new window.
18. **Opening preserves existing visibility:** opening a new window does not
    alter the minimized state of existing windows.
19. **Explicit restore:** restoring a minimized window is an explicit operation
    independent from applying or changing workspace mode.

Tests for exception candidates, maximize, the representation and mechanism of
minimize, persistence, failure semantics, and the other deliberately open
behavior are not acceptance criteria for this first version. The placement and
visibility separation in criteria 15 through 19 is required without implying
that minimize or restore operations are implemented by this version.
