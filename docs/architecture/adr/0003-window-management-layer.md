# ADR 0003: Dedicated Window-Management Layer

- Status: Accepted
- Date: 2026-09-26

## Context

Hyprland-specific interaction is necessary, but exposing it directly to the
shell or other clients would spread compositor details and policy throughout the
project. Desktop transitions also need consistent preservation of window state.

## Decision

CLEA has a dedicated window-management layer that owns window-management policy
and exposes compositor-independent contracts to clients. Direct Hyprland
interaction must remain behind CLEA-owned interfaces and a dedicated backend.

The layer must preserve fullscreen applications across ordinary desktop mode
changes. Each implemented mode transition must define and test its expected
window-state behavior.

This decision does not choose the backend language, transport, process model, or
specific Hyprland integration technique.

## Consequences

- Presentation code cannot issue direct Hyprland commands as a substitute for a
  window-management contract.
- Hyprland-specific behavior is isolated and can be replaced or tested at its
  boundary.
- Fullscreen preservation becomes an explicit acceptance criterion for relevant
  changes.
- New window-management features require documented policy and tests rather than
  ad hoc QML behavior.
