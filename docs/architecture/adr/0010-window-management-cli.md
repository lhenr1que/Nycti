# ADR 0010: Window Management CLI

- Status: Proposed
- Date: 2026-10-01

## Context

[ADR 0002](0002-shell-is-a-client.md) makes Shell, Settings, and CLI tools
clients of documented contracts, and
[ADR 0005](0005-window-management-runtime.md) leaves the name of a future CLI
open. [ADR 0009](0009-rename-project-to-nycti.md) renames the project and defers
the CLI name to this ADR. The daemon `nycti-windowd` serves protocol v1 on a Unix
socket (see the [protocol specification](../window-management-protocol-v1.md))
and was validated manually against a real Hyprland session, so a first client can
now exercise the protocol without a shell.

The CLI is a client. It is never an authority: it does not decide policy, does
not reach the compositor, and does not reproduce the planner. This ADR records
the choices for that client. It modifies no earlier ADR and no protocol
contract.

## Decision

### Name and command groups

The executable is `nycti`. Window Management commands live under the group `wm`,
so the name stays available for other components. Commands map one to one to the
nine protocol methods and are split into two groups:

```text
nycti [--json] [--timeout SECONDS] [--dry-run] wm query <status | get-default-mode | list-workspaces | get-workspace-mode WORKSPACE | list-windows>
nycti [--json] [--timeout SECONDS] [--dry-run] wm change <set-default-mode MODE | set-workspace-mode WORKSPACE MODE | clear-workspace-mode WORKSPACE | apply-workspace-mode WORKSPACE --yes>
```

`MODE` is `tiling` or `windows`, the protocol tokens. Options may appear before
or after the command words. `--help` prints the usage and exits with 0.

The `query` group is a separate type from the `change` group, so it can only
build the five read methods; no query command can become a request that changes
state. There is no `toggle` command: toggling would be a non-atomic composition
of a read, a set, and an apply, and the policy it implies belongs to the daemon.

### Opaque tokens

The user passes a workspace token exactly as `list-workspaces` printed it (for
example `w:2`, not `2`). The CLI only passes it on; the daemon decides whether it
is valid, and an unknown token is the protocol error `unknown_workspace`. The
client never infers, parses, or builds a token, as the protocol specification
requires.

Protocol v1 has no "active workspace". The CLI therefore cannot offer a command
such as "apply to the current workspace", which a future keyboard shortcut would
need. This is recorded as a debt.

### Confirmation of apply

`apply-workspace-mode` changes real windows; the setters change only the daemon's
memory. `--yes` is required for `apply-workspace-mode` and for no other command
(it is a usage error elsewhere). Without it the command exits with 2, explains
that it changes real windows, and does not connect, including together with
`--dry-run`. With it, a warning is written to standard error before the request
is sent.

### Dry run

`--dry-run` prints the JSON request that would be sent and does not connect. It
needs no daemon and no `XDG_RUNTIME_DIR`. There is no preview computed by the
client, because predicting what `apply-workspace-mode` would do would duplicate
the daemon's policy.

### Connection

The socket path is only `$XDG_RUNTIME_DIR/nycti/window-management.sock`, with a
variable that is set, non-empty, and absolute. There is no `--socket` flag and no
fallback in `/tmp`, as in the protocol specification. The CLI creates nothing;
the daemon owns the path. The tests inject the path into the library function.

Each command uses one connection: connect, send one request line, read one
response line, close. The timeouts are 5 seconds to connect and to write, and 10
seconds for the response; `--timeout` sets the response timeout (a positive
number of seconds, which may be fractional) and is a deadline for the whole line,
not for each read. A response line is limited to 8 MiB. The request correlation
ID is the fixed string `1`, because a connection carries one request.

The standard library has no connect timeout for Unix sockets, so the connection
is made on a helper thread that is waited for with `recv_timeout`. This is a
provisional workaround.

### Responses and failures

The response is checked before it is shown: it must be one LF-terminated JSON
object, `version` 1, an `id` equal to the one sent, and a boolean `ok` with a
`result` or an `error` of the expected shape. The result shape is selected by the
method of the request. Unknown response fields are ignored; unknown enum values
are not accepted. Each failure has its own message: the socket is absent (the
daemon is not running), the socket is stale (connection refused), permission is
denied, the response is malformed (invalid JSON, not an object, missing or
mistyped field, another version, another id, an unexpected result shape, a line
without LF, a line above the limit), or the daemon closed the connection without
responding. A failure after the request was sent, in a `change` command, adds
that the request may have taken effect and points to `wm query` to check.

### Output and exit codes (provisional)

By default the output is plain text with no stable format, in the order received
(no sorting, because sorting would mean interpreting tokens), for example:

```text
w:2  present  default=tiling  explicit=windows  effective=windows
win:7  workspace=w:2  floating  fullscreen=no  focused=yes
```

With `--json`, standard output receives the protocol `result` object on one
line. Errors always go to standard error; with `--json` the error line is
`{"error":{"code":"…","message":"…"}}`, where `code` is the protocol code for a
daemon error and a client-defined code otherwise. Exit codes:

| Code | Meaning |
| --- | --- |
| 0 | Success. |
| 1 | Internal failure of the CLI. |
| 2 | Incorrect usage (unknown command, missing or extra argument, invalid mode, `apply-workspace-mode` without `--yes`). |
| 3 | Daemon unavailable (no socket, stale socket, permission denied, unusable `XDG_RUNTIME_DIR`, connect failure). |
| 4 | The daemon answered with a protocol error; the code is printed and is in the JSON. |
| 5 | Communication failure (response timeout, connection closed without a response, malformed response). |

The text, the JSON error codes other than the protocol's, and the exit codes are
provisional and promise no stability.

### Implementation

- The executable is a second binary of the existing package `nycti-windowd`:
  `src/bin/nycti.rs` stays thin and the code lives in the library module
  `client`. The CLI does not use `hyprctl`, does not import the Hyprland backend,
  and does not decide policy; it speaks only protocol v1.
- Arguments are parsed with the standard library, with no new dependency. The
  parser is covered by a table of valid and invalid command lines.
- The protocol types gain `Deserialize` (additive; the wire format is unchanged),
  and `StatusResult.service` becomes a `Cow<'static, str>` so a client can own the
  value it reads. The serialized form is the same.

## Alternatives considered

- A separate crate for the client: deferred. It becomes worthwhile when a second
  client needs the same code (see debts).
- `clap` or another parsing library: rejected for now; the grammar is small and
  the standard library is enough, which keeps `Cargo.lock` unchanged.
- A `toggle` command, a client-side dry-run preview, and a `--socket` flag:
  rejected, as explained above and in the protocol specification.
- Reporting the token as a bare number: rejected; tokens are opaque.

## Consequences

- Shell, Settings, and scripts have a reference client and a way to exercise the
  daemon without owning policy.
- The `query` and `change` split and the `--yes` rule make a read command unable
  to move windows and an apply command impossible by accident.
- The output and exit codes may change without notice; scripts should prefer
  `--json` and the exit code until a stability promise is made.

## Debts and limitations (provisional)

- The name `nycti` and the placement inside the `nycti-windowd` package are
  provisional. Extracting the client into its own crate is a declared debt, to be
  done when a second client appears. The name Nycti has had no trademark search
  (ADR 0009).
- The connect timeout uses a helper thread, as a workaround for the missing
  standard-library support.
- Output and exit codes carry no stability promise.
- Tokens are not valid across a daemon restart, and protocol v1 has no active
  workspace, which blocks a keyboard-shortcut command until a future protocol
  version.
- There is no `toggle` command.
- The daemon has no read timeout, so a client that connects and sends nothing
  holds a worker thread until it closes (ADR 0007 debts).
- The effect of `apply-workspace-mode` on the real compositor is not covered by an
  automated test; only the manual script exercises it.

## Out of scope

This ADR does not decide shell integration, completions, a configuration file, a
manual page, packaging, a stable output format, or any change to the protocol.
