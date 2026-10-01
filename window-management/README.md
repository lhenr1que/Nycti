# Window Management

Window Management owns CLEA's window-management policy boundary, including the
interfaces and backend boundary for interaction with Hyprland. Presentation
components do not implement this policy directly.

This component is responsible for preserving window-state invariants, including
keeping fullscreen applications fullscreen during ordinary desktop mode
changes, and for allowing the compositor integration to be replaced without
coupling clients to it.

The initial `clea-windowd` implementation uses Rust, as established by
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
(signals), both still Proposed. The library implements the service authority,
the connection workers, and the coordinator. `clea-windowd` is a functional
executable: it binds the service socket, starts the coordinator, and stops on
`SIGTERM` or `SIGINT`. Its automated tests run it only against a fake Hyprland;
it has not been validated by the project against a real Hyprland session. Shell,
Settings, and packaging integration remain future work.

Normal tests use fake backends and local Unix sockets. Process-level tests start
the binary with a cleared environment and a fake Hyprland socket, and need
`/usr/bin/kill` to signal the child process. Three ignored tests
require real Hyprland; they are opt-in and are not part of the offline baseline.
See [CURRENT_STAGE.md](../CURRENT_STAGE.md) for exact validated commands,
toolchain, results, and known limitations.
