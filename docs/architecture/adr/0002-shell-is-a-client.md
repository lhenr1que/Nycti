# ADR 0002: The Shell Is a Client

- Status: Accepted
- Date: 2026-09-26

## Context

A desktop shell needs to display system state and initiate actions, but placing
system authority in the presentation layer couples visual code to policy and
makes both difficult to replace or test independently.

## Decision

The CLEA shell is a client, not an authority. It presents state and expresses
user intent through documented APIs or IPC contracts. Settings and CLI tools
follow the same client boundary.

QML must not directly implement window-management policy or bypass service
contracts to mutate system state.

The concrete IPC mechanism and backend implementation language are intentionally
not selected by this decision.

## Consequences

- System and window-management policy live outside presentation clients.
- Client-visible capabilities require documented contracts.
- Shell, Settings, and CLI can evolve or be replaced without owning provider
  internals.
- New shell features may require a service or window-management contract before
  UI work can be completed.
