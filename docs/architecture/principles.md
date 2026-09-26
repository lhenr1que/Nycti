# Architectural Principles

## 1. The shell is a client

The shell presents state and captures user intent. System authority belongs
behind documented service and window-management contracts, not in the shell.

## 2. Presentation does not own window policy

QML must not directly implement window-management policy. Presentation code may
request an outcome, but the dedicated window-management layer determines and
applies policy.

## 3. Hyprland is behind a backend boundary

All direct Hyprland interaction must be isolated behind CLEA-owned interfaces
and a dedicated backend. Clients must not depend on compositor-specific commands
or wire formats.

## 4. Client contracts are documented

Shell, Settings, and CLI consume documented APIs or IPC contracts. A contract
must describe behavior and compatibility without requiring clients to know the
provider's implementation details.

## 5. Configuration is versioned

Configuration must have an explicit schema and version. Changes must account for
validation and compatibility rather than relying on undocumented structure.

## 6. Caelestia is the primary shell upstream

Shell architecture and upstream maintenance should favor continued alignment
with Caelestia. CLEA-specific work should avoid unnecessary divergence.

## 7. Midnight Shell is a selective reference

Midnight Shell may inform individual features, but it is not a second upstream
to merge wholesale. Any port must be isolated, reviewed against CLEA's
boundaries, documented, and tested.

## 8. Preserve fullscreen state

Ordinary desktop mode changes must not cause fullscreen applications to lose
fullscreen. Window-management changes must treat this as a tested invariant.

## 9. Components are independently replaceable

Major components depend on explicit contracts rather than internal details.
Replacing one should not require unrelated components to be rewritten.

## 10. Non-trivial features require documentation and tests

Every non-trivial feature must document its behavior and boundaries and include
tests appropriate to its risks and contracts.
