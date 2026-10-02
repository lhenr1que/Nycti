# Window Management

Window Management owns Nycti's window-management policy boundary, including the
interfaces and backend boundary for interaction with Hyprland. Presentation
components do not implement this policy directly.

This component is responsible for preserving window-state invariants, including
keeping fullscreen applications fullscreen during ordinary desktop mode
changes, and for allowing the compositor integration to be replaced without
coupling clients to it.

The initial `nycti-windowd` implementation uses Rust, as established by
[ADR 0005](../docs/architecture/adr/0005-window-management-runtime.md).

From this directory, validate the package with:

```sh
cargo check
cargo test
cargo fmt --all -- --check
cargo clippy --all-targets
```

The library implements `WindowManager`, protocol v1 and its service, JSON Lines
transport, `FakeBackend`, the complete synchronous `HyprlandBackend`, and the
Unix runtime adapter. Hyprland reads, normalization, placement and focus actions
are implemented for the documented 0.56.2 target. Actions verify their
postconditions through fresh snapshots.

Mode setters change in-memory policy only; `apply_workspace_mode` explicitly
applies placement to current windows. Automatic event handling and persistence
are not implemented.

The multi-client execution model is selected by
[ADR 0006](../docs/architecture/adr/0006-window-management-daemon-execution.md)
and refined by [ADR 0007](../docs/architecture/adr/0007-window-management-daemon-coordinator.md)
(coordinator) and [ADR 0008](../docs/architecture/adr/0008-window-management-daemon-signals.md)
(signals), both Accepted. The library implements the service authority,
the connection workers, and the coordinator. `nycti-windowd` is a functional
executable: it binds the service socket, starts the coordinator, and stops on
`SIGTERM` or `SIGINT`. Its automated tests run it only against a fake Hyprland;
the maintainer validated it manually on a real Hyprland session on 2026-09-30. Shell,
Settings, and packaging integration remain future work.

## Command-line client

`nycti` is the command-line client of the daemon, a second binary of this
package ([ADR 0010](../docs/architecture/adr/0010-window-management-cli.md),
Proposed). It is a client only: it sends protocol v1 requests to the daemon's
socket (`$XDG_RUNTIME_DIR/nycti/window-management.sock`) and shows the answer. It
does not use `hyprctl` and decides no policy.

```sh
cargo build --release
./target/release/nycti wm query status
./target/release/nycti wm query list-workspaces
./target/release/nycti wm query list-windows
./target/release/nycti wm change set-workspace-mode w:2 windows
./target/release/nycti wm change apply-workspace-mode w:2 --yes
```

Commands are one per protocol method, in two groups: `wm query` (read only:
`status`, `get-default-mode`, `list-workspaces`, `get-workspace-mode WORKSPACE`,
`list-windows`) and `wm change` (`set-default-mode MODE`, `set-workspace-mode
WORKSPACE MODE`, `clear-workspace-mode WORKSPACE`, `apply-workspace-mode
WORKSPACE --yes`). `MODE` is `tiling` or `windows`. Pass a workspace token exactly
as `list-workspaces` printed it (for example `w:2`); it is opaque and only the
daemon decides whether it is valid.

The setters change only the daemon's in-memory policy. Only
`apply-workspace-mode` changes real windows, so it requires `--yes`; without it
the command exits with 2 and sends nothing.

Options, before or after the command: `--json` prints the protocol result as one
JSON line (errors become `{"error":{"code":"…","message":"…"}}` on standard
error), `--timeout SECONDS` sets the response timeout (default 10), `--dry-run`
prints the request without connecting, and `--help` prints the usage.

Exit codes: 0 success, 1 internal failure, 2 incorrect usage, 3 daemon
unavailable, 4 the daemon answered with a protocol error, 5 communication failure
(timeout, closed connection, malformed response). The output text and the exit
codes are provisional and promise no stability. There is no `toggle` command, and
protocol v1 has no "active workspace", so a workspace is always named by token.

Normal tests use fake backends and local Unix sockets. Process-level tests start
the daemon, and the `nycti` client against it, with a cleared environment and a
fake Hyprland socket; the daemon tests need `/usr/bin/kill` to signal the child
process. Three ignored tests
require real Hyprland; they are opt-in and are not part of the offline baseline.
See [CURRENT_STAGE.md](../CURRENT_STAGE.md) for exact validated commands,
toolchain, results, and known limitations.
