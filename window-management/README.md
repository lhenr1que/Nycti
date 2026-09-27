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
```

Read-only Hyprland IPC transport, JSON parsing, and snapshot normalization are
available. Hyprland actions and the complete `WindowBackend` implementation are
not implemented yet, nor is the client protocol.
An offline `FakeBackend` is available for backend-independent tests.
