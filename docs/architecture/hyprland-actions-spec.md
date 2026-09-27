# Hyprland Actions Specification

## Status and scope

This document specifies the first translation from CLEA's synchronous
`WindowBackend` action semantics to native Hyprland request-socket actions. It
covers exactly:

- `ensure_floating(window)`;
- `ensure_tiled(window)`; and
- `focus_window(window)`.

This specification applies initially and exclusively to:

```text
Hyprland 0.56.2
```

Compatibility with any earlier or later Hyprland version is not claimed. A
different version requires separate validation of its dispatcher syntax,
selectors, responses, and action semantics.

This document refines the Hyprland adapter without changing the
compositor-independent `WindowBackend` contract, the Window Management
functional contract, or the runtime ownership established by ADR 0005. The
Window Management core continues to express desired outcomes with opaque CLEA
identities. Hyprland-specific identities, payloads, sockets, and response
details remain private to the adapter.

## Relationship to the backend boundary

The translation boundary is:

```text
CLEA WindowBackend action
        |
        v
private Hyprland identity lookup
        |
        v
canonical Hyprland 0.56.2 socket1 dispatch
        |
        v
existing read-only snapshot pipeline
        |
        v
postcondition verification
        |
        v
Ok / UnknownWindow / ActionFailed
```

The core must not receive or construct a Hyprland selector, Lua expression,
native stable ID, address, or generic dispatcher request. The backend applies a
fixed translation for each `WindowBackend` operation.

## Native request transport

### Socket

Actions use the same native Hyprland request socket as the existing read-only
adapter:

```text
$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock
```

The path remains derived at runtime from `XDG_RUNTIME_DIR` and
`HYPRLAND_INSTANCE_SIGNATURE`. `hyprctl` is not a runtime dependency.

### Endpoint

Every action uses the request-socket endpoint:

```text
/dispatch
```

followed by a fixed `hl.dsp` expression. Actions must not use:

- `/eval`;
- `/repl`;
- a generic Lua evaluation facility; or
- a generic core-facing dispatcher facility.

The `/dispatch` endpoint is already the dispatch operation. Its expression is
the specific `hl.dsp` value to invoke; the payload does not wrap that expression
in `hl.dispatch(...)`.

### Connection lifecycle

Each action request uses one short, synchronous connection:

1. open a Unix stream to the request socket;
2. write exactly one canonical `/dispatch` request;
3. read its complete response;
4. close the connection promptly; and
5. if the response permits postcondition verification, obtain a fresh snapshot
   through the existing read-only pipeline.

The post-action snapshot continues to issue its existing read requests on their
own short connections. The first action implementation does not batch the
dispatch with those reads and does not introduce persistent connections,
timeouts, retries, asynchronous I/O, or an async runtime.

## Action identity

### Primary and only action selector

The first implementation uses `stableid:` as the primary and only selector for
all three actions. Its canonical format is:

```text
stableid:<lowercase-hexadecimal-without-0x>
```

The value is produced from the adapter's private numeric `NativeStableId(u64)`.
The adapter must format that number afresh as lowercase hexadecimal without a
`0x` prefix. It must not reuse the original JSON string received from Hyprland.

The resolution path is:

```text
CLEA WindowId
    |
    v
private Hyprland identity mapping
    |
    v
NativeStableId(u64)
    |
    v
lowercase hexadecimal formatting
    |
    v
stableid:<hex>
```

No client may supply a native stable ID or arbitrary selector. `stableId` must
not be exposed to the core or client protocol.

### Address is not an action fallback

The Hyprland address remains stored privately for snapshot correlation and
future event integration. It is not used as an action selector in the first
implementation and must not be tried automatically when `stableid:` does not
resolve.

This prohibition is intentional because an address:

- is an operational identity within a compositor instance;
- can become stale;
- has a theoretical reuse risk; and
- could make a silent fallback affect a different window.

If the stable ID cannot identify the target, the action fails according to this
specification. It does not retry with the historical address.

## Safe payload construction

Action payloads must not interpolate arbitrary strings controlled by a client.
The only variable payload component is lowercase hexadecimal produced from an
internal `u64`.

The following text components are private constants:

- `/dispatch`;
- the dispatcher name;
- the table field names;
- punctuation and quoting; and
- the canonical action string.

The backend must not expose a public `eval(String)`, `dispatch(String)`, or
equivalent generic command API to the core. This restricted construction means
the selector value cannot contain arbitrary Lua code.

## Canonical action translations

### `ensure_floating(window)`

The only canonical payload is:

```text
/dispatch hl.dsp.window.float({ window = "stableid:<id>", action = "enable" })
```

`<id>` is the lowercase hexadecimal representation of `NativeStableId`, without
`0x`.

The backend must emit only `action = "enable"`. It must not use `on`, `toggle`,
an absent `action` field, an empty action string, or any other alias or spelling.

Conceptual operation flow:

```text
WindowBackend::ensure_floating(WindowId)
    |
    v
resolve WindowId -> NativeStableId
    |
    v
send canonical /dispatch with action = "enable"
    |
    v
obtain a snapshot through the existing read-only pipeline
    |
    v
find the same NativeStableId and verify Floating
    |
    +-- missing ----------> UnknownWindow(original WindowId)
    +-- Floating ---------> Ok(())
    +-- not Floating -----> ActionFailed
```

### `ensure_tiled(window)`

The only canonical payload is:

```text
/dispatch hl.dsp.window.float({ window = "stableid:<id>", action = "disable" })
```

`<id>` is the lowercase hexadecimal representation of `NativeStableId`, without
`0x`.

The backend must emit only `action = "disable"`. It must not use `off`, `toggle`,
an absent `action` field, an empty action string, or any other alias or spelling.

Conceptual operation flow:

```text
WindowBackend::ensure_tiled(WindowId)
    |
    v
resolve WindowId -> NativeStableId
    |
    v
send canonical /dispatch with action = "disable"
    |
    v
obtain a snapshot through the existing read-only pipeline
    |
    v
find the same NativeStableId and verify Tiled
    |
    +-- missing ------> UnknownWindow(original WindowId)
    +-- Tiled --------> Ok(())
    +-- not Tiled ----> ActionFailed
```

### `focus_window(window)`

The only canonical payload is:

```text
/dispatch hl.dsp.focus({ window = "stableid:<id>" })
```

`<id>` is the lowercase hexadecimal representation of `NativeStableId`, without
`0x`.

Conceptual operation flow:

```text
WindowBackend::focus_window(WindowId)
    |
    v
resolve WindowId -> NativeStableId
    |
    v
send canonical /dispatch focus request
    |
    v
obtain a snapshot through the existing read-only pipeline
    |
    v
find the same NativeStableId and verify focused == true
    |
    +-- missing ----------> UnknownWindow(original WindowId)
    +-- focused ----------> Ok(())
    +-- not focused ------> ActionFailed
```

## `WindowId` lookup

Before constructing a payload, the backend resolves the opaque CLEA `WindowId`
through the Hyprland adapter's private identity mapping:

```text
WindowId -> NativeStableId
```

If the `WindowId` has never been known to this backend instance, the operation
returns:

```text
BackendError::UnknownWindow(window_id)
```

No request is sent to Hyprland in that case.

An ID that was known can become stale because the window may disappear after a
snapshot or between lookup and dispatch. Resolving a retained private mapping
does not prove that the window is still present. Liveness is established only
by the post-action snapshot.

## Dispatcher response semantics

A successful-looking `/dispatch` response is not sufficient to report CLEA
action success. In the audited Hyprland 0.56.2 implementation,
`hl.dsp.window.float` can report success when a stale selector matched no window
and changed nothing. Therefore:

```text
dispatch success != CLEA postcondition success
```

The first implementation distinguishes these categories:

| Condition | Backend result |
|---|---|
| Socket path, connect, write, or read failure | `BackendError::CompositorUnavailable` |
| Explicit dispatcher error that unambiguously means the target no longer exists | `BackendError::UnknownWindow(original_window_id)` |
| Other explicit dispatcher error | `BackendError::ActionFailed` |
| Apparent dispatcher success | Continue to mandatory postcondition verification |

The implementation must not initially depend on a complete catalog of
Hyprland error strings. Exact parsing should be limited to response forms that
are required and covered by implementation tests. Unknown explicit dispatcher
errors map to `ActionFailed` rather than being treated as success.

## Mandatory postcondition verification

After every action whose dispatcher response permits continued verification,
the backend obtains exactly one new read-only snapshot. There is no retry in the
first implementation.

The verification snapshot must use the existing stateful
`HyprlandSnapshotSource` and its existing transport, wire parsing, correlation,
and normalization pipeline. Action verification must not introduce a second
JSON parser, a second identity allocator, or a second source of normalized
truth.

Using the same stateful source ensures that:

- a native stable ID that remains present retains its CLEA `WindowId`;
- newly observed addresses continue to update the private correlation state;
- workspaces use the same normalization and identity allocation; and
- action verification observes the same normalized placement and focus model
  used by ordinary reads.

The target is located internally by the same numeric native stable ID used to
construct the request. This native comparison remains inside the adapter; the
normalized observation exposed to the core still contains only the opaque CLEA
`WindowId`.

If the snapshot transport fails, the operation returns
`CompositorUnavailable`. If parsing or normalization cannot produce a valid
snapshot, the existing `ObservedStateUnavailable` or
`InconsistentObservedState` classification is preserved. A successful snapshot
that no longer contains the target stable ID yields `UnknownWindow`.

### Floating postcondition

After `ensure_floating`:

1. obtain one fresh snapshot;
2. locate the same numeric native stable ID;
3. if absent, return `UnknownWindow(original_window_id)`;
4. if present with `WindowPlacement::Floating`, return `Ok(())`; and
5. otherwise return `ActionFailed`.

### Tiled postcondition

After `ensure_tiled`:

1. obtain one fresh snapshot;
2. locate the same numeric native stable ID;
3. if absent, return `UnknownWindow(original_window_id)`;
4. if present with `WindowPlacement::Tiled`, return `Ok(())`; and
5. otherwise return `ActionFailed`.

### Focus postcondition

After `focus_window`:

1. obtain one fresh snapshot;
2. locate the same numeric native stable ID;
3. if absent, return `UnknownWindow(original_window_id)`;
4. if present with `focused == true`, return `Ok(())`; and
5. otherwise return `ActionFailed`.

The backend never reports success solely because Hyprland returned an apparent
dispatcher success response.

## Races and non-atomicity

The action lifecycle is inherently non-atomic:

```text
previous snapshot
    |
    v
WindowId lookup
    |
    v
dispatch
    |
    v
post-action snapshot
```

The first implementation has no compositor lock, transaction, rollback, retry,
or compare-and-swap behavior. In particular:

- if the target disappears during the interval and the successful verification
  snapshot no longer contains its stable ID, return `UnknownWindow`;
- if the target remains but the requested placement or focus is not observed,
  return `ActionFailed`; and
- do not silently issue another action.

These outcomes are observations after a race, not proof that no transient
change occurred between requests.

## Declarative idempotence

Hyprland 0.56.2 provides explicit placement actions with the required
idempotent meaning:

```text
Floating + enable  -> Floating
Tiled    + disable -> Tiled
```

The backend must not:

- inspect placement to decide whether to use toggle;
- translate an ensure operation into toggle;
- invert placement as a recovery technique; or
- issue a second placement action to emulate idempotence.

Postcondition verification confirms the result and detects staleness. It does
not implement idempotence; the canonical Hyprland action already supplies that
semantic.

## Focus and workspace activation

`focus_window(window)` requests full focus for the identified window. In the
audited Hyprland 0.56.2 implementation, focusing a window on another workspace
can activate that window's workspace. This is a confirmed implementation effect
and is accepted as a consequence of focus in the first version.

The backend must not hide this effect and must not issue a separate workspace
movement or workspace-selection action. Workspace activation caused by focus
does not mean:

- moving the window;
- changing its workspace membership;
- changing any CLEA workspace mode; or
- transferring mode state between workspaces.

CLEA workspace mode remains owned by the workspace and is not modified by a
focus operation.

## Fullscreen relationship

No action in this specification emits a fullscreen command. The audited
Hyprland placement dispatcher accepts a specific window and does not require an
explicit fullscreen exit before changing placement.

The Window Management policy already produces `WindowPlacementAction::Keep`
for ordinary mode transitions involving fullscreen windows, so those
transitions normally do not request a placement action for a fullscreen target.
Regardless, the backend must never implement this sequence:

```text
fullscreen off
placement action
fullscreen on
```

It must not add that sequence as a fallback or recovery strategy. The backend
exposes no fullscreen action through the first `WindowBackend` contract.

## Testability requirements

The action implementation must be testable without a real Hyprland instance.
Its request transport must be replaceable or simulated using the same approach
as the existing read-only transport tests. Tests may provide exact dispatcher
responses and exact post-action read responses while recording every request.

Future automated tests must demonstrate at least that:

1. `ensure_floating` constructs exactly the canonical payload with `enable`;
2. `ensure_tiled` constructs exactly the canonical payload with `disable`;
3. `focus_window` constructs exactly the canonical focus payload;
4. the stable ID is formatted as lowercase hexadecimal without `0x`;
5. no action payload uses `address:`;
6. no action payload uses `toggle`;
7. an unknown CLEA `WindowId` sends no compositor request;
8. dispatcher success followed by observed `Floating` returns `Ok(())` for
   `ensure_floating`;
9. dispatcher success followed by observed `Tiled` returns `Ok(())` for
   `ensure_tiled`;
10. dispatcher success followed by the target observed as focused returns
    `Ok(())` for `focus_window`;
11. a target that disappears after dispatch returns `UnknownWindow`;
12. incorrect placement after dispatch returns `ActionFailed`;
13. focus not obtained after dispatch returns `ActionFailed`;
14. postcondition verification uses the existing stateful snapshot and
    normalization pipeline;
15. no action toggles fullscreen;
16. no action uses `/eval`;
17. no action executes `hyprctl`; and
18. no action automatically falls back to `address:`.

Tests should also cover transport failure and explicit dispatcher-error
classification without requiring a complete parser for every possible
Hyprland error message.

## Future opt-in live test

Implementation must be validated with mock transports before any live action
test is introduced. A real action smoke test may be added only after review and
must remain explicitly opt-in.

That future test must:

- use a window explicitly chosen for testing;
- record its initial relevant state;
- perform a controlled action;
- verify the observed result; and
- explicitly restore the initial state when restoration is safe.

This document neither implements nor authorizes execution of that test.

## Deliberately out of scope

The following remain outside this specification:

- socket2;
- an event loop;
- asynchronous I/O or Tokio;
- retries;
- timeout policy;
- minimize or restore;
- maximize;
- geometry or cascade placement;
- moving windows between workspaces;
- persistence;
- compatibility with Hyprland versions other than 0.56.2;
- generic Lua evaluation;
- a generic public dispatcher interface;
- rollback or compositor transactions; and
- implementation or execution of a live action test.

## Acceptance criteria

This specification is satisfied by a future implementation when:

1. the three `WindowBackend` operations use the exact canonical payloads in
   this document;
2. action targets are derived only from private numeric `NativeStableId` values;
3. unknown CLEA identities are rejected before transport;
4. apparent dispatch success is followed by one snapshot-based postcondition
   check;
5. stale targets and unmet postconditions map to the specified backend errors;
6. placement actions remain declarative and never use toggle;
7. `address:` is not used as an automatic action fallback;
8. action verification reuses the existing stateful snapshot source;
9. focus-induced workspace activation remains visible but does not alter CLEA
   workspace modes or membership;
10. no fullscreen command is emitted;
11. the implementation has no runtime dependency on `hyprctl`; and
12. all mock-based requirements above pass without a running compositor.

## Remaining implementation questions

This specification intentionally leaves these details for implementation and
tests without weakening the required action semantics:

1. the smallest internal transport abstraction that supports both canonical
   action requests and the existing read-only mocks;
2. the exact validated wire forms for successful and failed `/dispatch`
   responses;
3. which explicit Hyprland error responses are sufficiently unambiguous to map
   directly to `UnknownWindow` rather than `ActionFailed`;
4. the internal map structure needed for efficient `WindowId -> NativeStableId`
   lookup while preserving transactional snapshot updates; and
5. error precedence when a dispatcher response and the subsequent observation
   fail independently.

None of these questions permits `/eval`, toggle semantics, retries, address
fallback, duplicated snapshot parsing, or exposure of native identities.
