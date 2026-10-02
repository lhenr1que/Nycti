# Nycti Window Management Protocol v1

## Status and scope

This document specifies version 1 of the client protocol for `nycti-windowd`.
The protocol is used by the Nycti Shell, Settings, and the `nycti` command-line
client ([ADR 0010](adr/0010-window-management-cli.md), Proposed) to query and
change Window Management state without owning policy or communicating directly
with a compositor.

The architectural boundary is:

```text
Shell / Settings / CLI
        |
        | Nycti Window Management Protocol v1
        v
nycti-windowd
        |
        v
WindowManager
        |
        v
WindowBackend
```

`nycti-windowd` is the authority for the default workspace mode, explicit
workspace modes, effective-mode resolution, and application of placement
policy. Clients request operations and display results; they do not reproduce
the planner or issue compositor commands.

This protocol is compositor-independent. Compositor socket paths, native
workspace IDs, native window IDs, selectors, commands, wire responses, and
diagnostics are not protocol fields.

## Transport

Protocol v1 uses:

- one per-user Unix domain socket;
- UTF-8 JSON text;
- JSON Lines framing; and
- the mandatory protocol version `1` in every request and response.

The socket path is:

```text
$XDG_RUNTIME_DIR/nycti/window-management.sock
```

The daemon creates the `nycti` parent directory when necessary. It must derive
the complete path from the current process environment. It must not use `/tmp`,
hardcode a UID, or reuse a compositor socket.

If `XDG_RUNTIME_DIR` is missing, empty, not absolute, or otherwise unusable,
the daemon cannot create its protocol listener and must fail startup clearly.
Protocol v1 defines no fallback directory.

D-Bus is not part of protocol v1.

## Framing and connection behavior

Each request and each response is exactly one JSON object serialized on one
line and terminated by a line-feed byte (`\n`). Literal newlines inside JSON
strings must use normal JSON escaping and therefore do not delimit messages.

A connection may carry multiple request lines. The initial daemon is
synchronous on each connection:

1. it reads one complete request line;
2. it emits exactly one response line for that request; and
3. it emits responses in the same order as requests on that connection.

Protocol v1 does not define out-of-order responses. Scheduling or ordering
between separate client connections is not specified.

An invalid JSON line produces an `invalid_request` response. When no valid
correlation ID can be recovered, that response uses `"id": null`. The daemon
must remain able to process subsequent complete lines on the same connection.
A framing failure, transport failure, or incomplete final line may terminate
the connection without a response for the incomplete message.

Protocol v1 defines no normative maximum line length. Implementations must
handle resource exhaustion safely, but a concrete operational limit is not
specified here.

## JSON conventions

All protocol property names and string enum values are case-sensitive.
Requests use JSON objects; arrays, scalars, and `null` are not valid top-level
requests.

The canonical workspace mode values are exactly:

```json
"tiling"
"windows"
```

Protocol v1 accepts no aliases, alternative capitalization, or compositor
terms for these values.

The canonical placement values are exactly:

```json
"tiled"
"floating"
```

## Opaque identities

Workspace and window identities are non-empty JSON strings issued by
`nycti-windowd`. Examples in this document use:

```json
{
  "workspace_id": "w:1",
  "window_id": "win:7"
}
```

The complete string is opaque. Clients may compare it for equality, retain it
for the daemon instance's applicable identity lifetime, and return it in later
requests. Clients must not parse, increment, synthesize, or infer meaning from
the prefix or suffix. The examples do not require the daemon's internal IDs to
remain integers or require future emitted tokens to expose an integer.

An ID has no promised meaning across daemon restart, compositor restart, or a
new user session. Clients obtain current IDs from `list_workspaces` and
`list_windows` rather than constructing them.

The protocol never exposes or accepts as an identity:

- a compositor-native workspace ID;
- a compositor-native stable ID or address;
- a pointer;
- a process ID;
- a title or application class; or
- a compositor selector.

A `workspace_id` or `window_id` method parameter with the wrong JSON type or an
empty string is `invalid_params`. A well-formed opaque token that the daemon
cannot resolve is `unknown_workspace` or `unknown_window`, as appropriate.

## Request envelope

Every request has exactly this top-level shape:

```json
{
  "version": 1,
  "id": "client-correlation-id",
  "method": "method_name",
  "params": {}
}
```

The fields are:

- `version`: required JSON integer; it must be `1`;
- `id`: required non-empty string selected by the client;
- `method`: required non-empty string naming one v1 method; and
- `params`: required field; for a known method, its value must be an object
  containing exactly the fields defined for that method.

The correlation ID is opaque to the daemon. It need not be a UUID and does not
identify a workspace, window, connection, or user. The daemon echoes it in the
response. Clients are responsible for choosing IDs useful for correlation;
protocol v1 does not require the daemon to reject duplicate correlation IDs.

`invalid_request` is reserved for structural envelope failures, including:

- invalid JSON;
- a top-level value that is not an object;
- missing `version`, or a `version` with the wrong JSON type;
- missing, empty, or non-string `id`;
- missing, empty, or non-string `method` when validating a version 1 envelope;
- missing `params`; and
- unknown top-level fields in a version 1 request.

The presence of `params` is structural, but its value is method input. A missing
`params` field is therefore `invalid_request`. A present `params` value that is
not a JSON object is `invalid_params`. For a known method, a `params` object
with missing or extra fields, incorrect field types, invalid IDs, or invalid
enum values is also `invalid_params`.

## Request validation precedence

The first implementation validates each complete request line in this order:

1. Parse the line as JSON. Failure produces `invalid_request`, with `"id":
   null` when no valid correlation ID can be recovered.
2. Require a top-level JSON object. Any other top-level value produces
   `invalid_request`.
3. Recover and validate `id` and `version`, and require the minimum structural
   fields `method` and `params` to be present. A structural failure produces
   `invalid_request`.
4. If `version` is a valid integer other than `1`, produce
   `unsupported_version`. Do not validate `method` or `params` against version
   1 schemas.
5. For `version == 1`, validate the strict version 1 envelope, including the
   type and non-empty value of `method` and the absence of unknown top-level
   fields. Failure produces `invalid_request`.
6. Validate `method`. An unknown method produces `unknown_method`; its `params`
   value is not interpreted against a nonexistent method schema.
7. Validate `params` for the known method. A non-object `params` value or
   invalid method parameters produces `invalid_params`.
8. Only after all protocol validation succeeds, invoke the service handler.

This precedence makes error classification deterministic. For an unsupported
version, the daemon validates no version 1 method-specific fields beyond the
minimum structure needed to produce a safely correlated response. This rule
does not introduce support for any version other than version 1.

## Response envelopes

### Success

A successful response has:

```json
{
  "version": 1,
  "id": "client-correlation-id",
  "ok": true,
  "result": {}
}
```

`id` exactly echoes the request correlation ID. `result` has the method-specific
shape defined below. A success response does not contain `error`.

### Error

An error response has:

```json
{
  "version": 1,
  "id": "client-correlation-id",
  "ok": false,
  "error": {
    "code": "invalid_params",
    "message": "mode must be either tiling or windows"
  }
}
```

An error response does not contain `result`. `message` is human-readable and is
not stable API. Clients must branch on `code` and must not parse `message`.

If a request line cannot provide a valid string correlation ID, the error
response uses:

```json
"id": null
```

Successful responses always have a string `id`.

## Methods

Protocol v1 defines exactly these methods:

| Method | Purpose | Placement side effect |
|---|---|---|
| `status` | Identify the service and protocol version | No |
| `get_default_mode` | Read the current default mode | No |
| `set_default_mode` | Change the in-memory default mode | No |
| `list_workspaces` | List known workspace observations and mode resolutions | No |
| `get_workspace_mode` | Read one workspace's mode resolution | No |
| `set_workspace_mode` | Set one explicit in-memory workspace mode | No |
| `clear_workspace_mode` | Remove one explicit workspace mode | No |
| `apply_workspace_mode` | Apply one workspace's effective mode to current windows | Yes |
| `list_windows` | List normalized current window observations | No |

There is no generic `command`, `dispatch`, `eval`, or backend passthrough
method.

### `status`

Parameters:

```json
{}
```

Result:

```json
{
  "protocol_version": 1,
  "service": "nycti-windowd"
}
```

This result identifies a reachable protocol service. It does not expose the
active backend, compositor version, compositor socket, or other backend
metadata. It does not by itself guarantee that compositor observations are
currently available.

### `get_default_mode`

Parameters:

```json
{}
```

Result:

```json
{
  "mode": "tiling"
}
```

### `set_default_mode`

Parameters:

```json
{
  "mode": "windows"
}
```

Result:

```json
{
  "mode": "windows"
}
```

This method changes only the daemon's in-memory default workspace mode. It does
not apply placement policy to any workspace or window. Workspaces without an
explicit mode immediately resolve to the new default logically, but their
current window placement is unchanged until `apply_workspace_mode` is requested
for each relevant workspace.

### `list_workspaces`

Parameters:

```json
{}
```

Result:

```json
{
  "workspaces": [
    {
      "workspace_id": "w:1",
      "present": true,
      "default_mode": "tiling",
      "explicit_mode": null,
      "effective_mode": "tiling"
    },
    {
      "workspace_id": "w:2",
      "present": true,
      "default_mode": "tiling",
      "explicit_mode": "windows",
      "effective_mode": "windows"
    }
  ]
}
```

The list contains workspaces known through the daemon's normalized backend
boundary. Array order is unspecified. `explicit_mode` is either `null`,
`"tiling"`, or `"windows"`. The other mode fields are always present.

The response does not contain a native workspace ID, workspace name, monitor,
layout, or compositor metadata.

### `get_workspace_mode`

Parameters:

```json
{
  "workspace_id": "w:2"
}
```

Result:

```json
{
  "workspace_id": "w:2",
  "default_mode": "tiling",
  "explicit_mode": "windows",
  "effective_mode": "windows"
}
```

An unresolvable token returns `unknown_workspace`. Querying mode resolution does
not apply placement.

### `set_workspace_mode`

Parameters:

```json
{
  "workspace_id": "w:2",
  "mode": "windows"
}
```

Result:

```json
{
  "workspace_id": "w:2",
  "default_mode": "tiling",
  "explicit_mode": "windows",
  "effective_mode": "windows"
}
```

This method establishes or replaces the workspace's explicit in-memory mode. It
does not inspect windows and does not apply placement. Requesting the same mode
again leaves the same logical resolution and still performs no placement
action.

### `clear_workspace_mode`

Parameters:

```json
{
  "workspace_id": "w:2"
}
```

Result, assuming a `tiling` default:

```json
{
  "workspace_id": "w:2",
  "default_mode": "tiling",
  "explicit_mode": null,
  "effective_mode": "tiling"
}
```

This method removes the explicit in-memory override. It does not inspect windows
and does not apply placement. Clearing an already absent override succeeds with
the inherited resolution.

### `apply_workspace_mode`

Parameters:

```json
{
  "workspace_id": "w:2"
}
```

Result:

```json
{
  "workspace_id": "w:2",
  "effective_mode": "windows"
}
```

This is the only protocol v1 method that applies workspace placement policy. It
resolves the workspace's effective mode, confirms that the workspace is
currently present, observes the current windows in that workspace, plans each
placement through the Window Management core, and requests only the necessary
declarative backend actions.

A successful result means the applicable backend actions completed according
to the backend contract. It does not report a changed-window count because the
current `WindowManager` does not produce one.

Application is synchronous and non-transactional. On the first backend error,
the method returns the mapped protocol error without retry or rollback. Actions
completed before that failure may remain applied.

If the workspace token cannot be resolved, or the workspace is known but not
currently present when application begins, the method returns
`unknown_workspace`.

### `list_windows`

Parameters:

```json
{}
```

Result:

```json
{
  "windows": [
    {
      "window_id": "win:7",
      "workspace_id": "w:2",
      "placement": "floating",
      "fullscreen": false,
      "focused": true
    }
  ]
}
```

Array order is unspecified. Each entry contains only normalized Nycti state:

- opaque `window_id`;
- opaque containing `workspace_id`;
- `placement`, exactly `"tiled"` or `"floating"`;
- boolean `fullscreen`; and
- boolean `focused`.

The response does not contain native IDs, native addresses, selectors, title,
class, process ID, geometry, monitor, or other compositor-specific data.

## State changes versus placement application

Protocol v1 deliberately separates policy configuration from compositor side
effects.

These methods change or clear in-memory policy only:

```text
set_default_mode
set_workspace_mode
clear_workspace_mode
```

They never apply placement automatically.

Only this method applies the effective policy to windows:

```text
apply_workspace_mode
```

Clients that want to change a workspace's explicit mode and immediately apply
it issue two requests in order: `set_workspace_mode`, then
`apply_workspace_mode`. If the setter succeeds and application fails, the new
logical mode remains configured. Protocol v1 defines no transaction combining
the two operations.

Changing the default does not implicitly apply every inheriting workspace.
Clearing an override does not implicitly re-place that workspace's windows.

## Snapshot scope and future windows

`apply_workspace_mode` operates explicitly on the current observations obtained
during that operation. Protocol v1 does not promise automatic policy
application when a window is later created or moved between workspaces.

There are no event subscriptions, notifications, event-stream exposure, or
reconciliation-loop controls in v1. Automatic reaction to future state changes
requires a separately specified runtime event and reconciliation design.

## Error codes

The `error.code` vocabulary is small, stable, and compositor-independent:

| Code | Meaning |
|---|---|
| `invalid_request` | The line is invalid JSON, the top level is not an object, or the structural envelope is invalid, including a missing `params` field. |
| `unsupported_version` | `version` is a valid integer but is not supported by this endpoint. |
| `unknown_method` | `method` is a valid string but is not a protocol v1 method. |
| `invalid_params` | For a known method, present `params` is not an object or has missing or extra fields, incorrect types, invalid IDs, or invalid enum values. |
| `unknown_workspace` | The workspace token cannot be resolved, or an operation requiring presence targets a workspace that is not present. |
| `unknown_window` | A window token cannot be resolved or identifies a stale window. No current v1 method targets a window; the code is reserved for complete backend-error mapping. |
| `compositor_unavailable` | The configured backend cannot currently communicate with its compositor. |
| `action_failed` | A requested backend action did not establish its required outcome. |
| `observed_state_unavailable` | A usable current observation could not be obtained. |
| `inconsistent_observed_state` | Observed state was structurally valid enough to inspect but internally incompatible or contradictory. |
| `internal_error` | An unexpected daemon failure has no safer public classification. |

Protocol validation errors are produced before invoking the service operation.
Backend errors map conceptually as follows:

| Backend category | Protocol code |
|---|---|
| `BackendError::UnknownWorkspace` | `unknown_workspace` |
| `BackendError::UnknownWindow` | `unknown_window` |
| `BackendError::CompositorUnavailable` | `compositor_unavailable` |
| `BackendError::ActionFailed` | `action_failed` |
| `BackendError::ObservedStateUnavailable` | `observed_state_unavailable` |
| `BackendError::InconsistentObservedState` | `inconsistent_observed_state` |

Messages must describe the client-visible failure without exposing backend
commands, native selectors, socket paths, raw responses, or implementation
diagnostics. Unexpected implementation failures map to `internal_error` and
must not disclose sensitive internals.

## Complete JSON Lines examples

Each JSON object below occupies one complete line on the wire.

### 1. Status

Request:

```json
{"version":1,"id":"req-1","method":"status","params":{}}
```

Response:

```json
{"version":1,"id":"req-1","ok":true,"result":{"protocol_version":1,"service":"nycti-windowd"}}
```

### 2. List workspaces

Request:

```json
{"version":1,"id":"req-2","method":"list_workspaces","params":{}}
```

Response:

```json
{"version":1,"id":"req-2","ok":true,"result":{"workspaces":[{"workspace_id":"w:1","present":true,"default_mode":"tiling","explicit_mode":null,"effective_mode":"tiling"},{"workspace_id":"w:2","present":true,"default_mode":"tiling","explicit_mode":"windows","effective_mode":"windows"}]}}
```

### 3. Set workspace to Windows Mode

Request:

```json
{"version":1,"id":"req-3","method":"set_workspace_mode","params":{"workspace_id":"w:2","mode":"windows"}}
```

Response:

```json
{"version":1,"id":"req-3","ok":true,"result":{"workspace_id":"w:2","default_mode":"tiling","explicit_mode":"windows","effective_mode":"windows"}}
```

This response confirms only the logical state change. No placement action has
occurred.

### 4. Apply workspace mode

Request:

```json
{"version":1,"id":"req-4","method":"apply_workspace_mode","params":{"workspace_id":"w:2"}}
```

Response:

```json
{"version":1,"id":"req-4","ok":true,"result":{"workspace_id":"w:2","effective_mode":"windows"}}
```

### 5. Clear workspace override

Request:

```json
{"version":1,"id":"req-5","method":"clear_workspace_mode","params":{"workspace_id":"w:2"}}
```

Response:

```json
{"version":1,"id":"req-5","ok":true,"result":{"workspace_id":"w:2","default_mode":"tiling","explicit_mode":null,"effective_mode":"tiling"}}
```

This response confirms only the logical state change. Applying the inherited
mode requires a separate `apply_workspace_mode` request.

### 6. Unknown workspace

Request:

```json
{"version":1,"id":"req-6","method":"get_workspace_mode","params":{"workspace_id":"w:missing"}}
```

Response:

```json
{"version":1,"id":"req-6","ok":false,"error":{"code":"unknown_workspace","message":"workspace is not known"}}
```

### 7. Invalid parameters

Request:

```json
{"version":1,"id":"req-7","method":"set_default_mode","params":{"mode":"floating"}}
```

Response:

```json
{"version":1,"id":"req-7","ok":false,"error":{"code":"invalid_params","message":"mode must be either tiling or windows"}}
```

## Safety properties

- Messages are single-line JSON and use normal JSON string escaping.
- Clients send typed method parameters, never compositor or scripting commands.
- There is no generic command execution surface.
- Workspace mode controls placement, not visibility.
- Protocol v1 does not expose actions for fullscreen, minimize, restore,
  maximize, geometry, or moving windows.
- Protocol v1 defines no retry or rollback behavior for placement application.
- Native backend diagnostics are not stable protocol errors and are not parsed
  by clients.

## Protocol evolution

`version` is mandatory in every request. A valid integer version other than `1`
receives `unsupported_version`. Missing versions and versions with the wrong
JSON type receive `invalid_request`.

Incompatible request or response changes require a new protocol version.
Protocol v1 request envelopes and method parameters are strict: clients must not
send fields not defined for v1.

Future implementations may add response fields in a backward-compatible way.
Clients implementing v1 should ignore response object fields they do not
understand while continuing to require and validate the v1 fields they use.
Clients must not silently accept unknown enum values as aliases.

## Testability requirements

### Service observation boundary

The protocol implementation needs compositor-independent observations for
`list_workspaces` and `list_windows`. The implementation may add observation
operations to `WindowManager`, conceptually equivalent to:

```text
WindowManager::list_workspaces()
WindowManager::list_windows()
```

These operations provide normalized observations through the backend already
owned by the manager. They must not:

- expose `WindowBackend`;
- expose `HyprlandBackend` or any other concrete compositor backend;
- return compositor-native types;
- create a second owner of the backend; or
- allow the service handler to bypass `WindowManager`.

The normative dependency flow remains:

```text
service handler
    |
    v
WindowManager
    |
    v
WindowBackend
```

It must never become a direct `service handler -> backend` path. These
observation operations are a future implementation need, not a change to the
wire protocol. Their concrete Rust names may differ if this boundary and
ownership model are preserved.

### Parser and service tests

Protocol parsing and dispatch must be testable without a real compositor, Unix
socket, or daemon process. The preferred test boundary is:

```text
JSON line
    |
    v
protocol parser
    |
    v
typed request
    |
    v
service handler
    |
    v
WindowManager<FakeBackend>
    |
    v
typed response
    |
    v
JSON line
```

Socket listener, framing, filesystem path, and multi-line connection behavior
are tested separately from service semantics. Protocol tests must not require a
real compositor or expose backend-specific fields.

## Deliberately out of scope

Protocol v1 does not decide or provide:

- Tokio or another async runtime;
- implementation of the threading and multi-connection model selected by
  [ADR 0006](adr/0006-window-management-daemon-execution.md);
- a systemd user unit;
- socket activation;
- D-Bus;
- authentication beyond Unix filesystem permissions;
- event subscriptions or notifications;
- persistence format or policy persistence;
- automatic new-window handling;
- automatic moved-window handling;
- a reconciliation loop;
- minimize, restore, or maximize;
- geometry or cascade placement;
- moving windows between workspaces;
- a focus method;
- generic compositor commands; or
- backend event-socket exposure.

## Remaining questions

Rust protocol types, the service handler, and in-memory external token
allocation are implemented. The
[Unix runtime specification](window-management-unix-runtime.md) resolves stale
socket cleanup and permission modes, and
[ADR 0006](adr/0006-window-management-daemon-execution.md) selects threading,
single-authority ownership, and multi-client ordering. Its coordinator is
implemented in the `daemon` module, as recorded by
[ADR 0007](adr/0007-window-management-daemon-coordinator.md) (Proposed).

The following remain open:

1. retirement of historical external opaque ID tokens;
2. operational resource limits;
3. supervision of the daemon process, for example by a service manager;
4. persistence of default and explicit modes; and
5. future event-driven reconciliation and notification protocols.

None of these questions permits moving policy into clients, exposing backend
commands, or weakening the explicit separation between mode setters and
`apply_workspace_mode`.

## Acceptance criteria

A future protocol v1 implementation is acceptable when tests demonstrate that:

1. valid requests and responses round-trip as one JSON object per line;
2. every request requires integer `version: 1` and unsupported versions produce
   `unsupported_version`;
3. only `"tiling"` and `"windows"` are accepted as workspace modes;
4. workspace and window IDs are JSON strings treated as opaque tokens;
5. no response exposes backend-native identities, selectors, commands, socket
   paths, or backend-specific fields;
6. `set_default_mode`, `set_workspace_mode`, and `clear_workspace_mode` change
   logical state without requesting placement actions;
7. `apply_workspace_mode` explicitly invokes current-window placement policy;
8. normalized workspace and window listings have the shapes specified here;
9. every `BackendError` variant maps to its specified stable protocol code;
10. invalid JSON and invalid envelopes produce `invalid_request` without making
    the connection unable to process a subsequent valid line;
11. invalid method parameters and mode aliases produce `invalid_params`;
12. multiple request lines receive exactly one response each in request order;
13. correlation IDs are echoed without interpretation;
14. protocol parser and service tests run through `WindowManager<FakeBackend>`
    without a real socket or compositor;
15. socket transport and JSON/service behavior are independently testable; and
16. no generic dispatcher, evaluator, scripting, or compositor-command surface
    exists.
