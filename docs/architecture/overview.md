# Architecture Overview

## Purpose

CLEA is a modular desktop environment for Hyprland. The architecture separates
presentation, user configuration, window-management policy, and system
capabilities so that no visual client becomes the authority for the desktop.

This document describes project-wide component boundaries. Languages,
transports, and process models are not universal CLEA requirements; individual
components may select them through component-specific architecture decisions.

## Components

| Component | Responsibility |
| --- | --- |
| `shell/` | Visual and interactive desktop surface, with Caelestia as its primary upstream. |
| `settings/` | User-facing configuration client. |
| `window-management/` | Window-management policy and the interfaces/backends that isolate direct Hyprland interaction. |
| `services/` | Non-visual desktop capabilities exposed through documented contracts. |
| `config/` | Versioned configuration schemas and compatibility contracts. |
| `packaging/` | Distribution metadata and component integration for supported packages. |
| `tests/` | Verification of behavior, contracts, and architectural boundaries. |
| `docs/` | Architecture, decisions, and feature documentation. |

## Authority and communication

Shell, Settings, and CLI are clients. They request capabilities through
documented APIs or IPC contracts rather than directly changing system state.
Transports are selected per component and remain replaceable implementation
boundaries rather than sources of system policy.

Direct interaction with Hyprland belongs behind interfaces and a dedicated
backend in the window-management boundary. QML may present state and express
user intent, but it must not implement window-management policy.

Conceptually, dependencies point inward through documented contracts:

```text
Shell / Settings / CLI
          |
          | documented APIs or IPC
          v
Services / Window Management
          |
          | dedicated backend boundary
          v
      Hyprland / system
```

This diagram establishes responsibility, not a required runtime topology.

For Window Management, [ADR 0005](adr/0005-window-management-runtime.md)
selects an independent per-user `clea-windowd` process, initially implemented in
Rust, with a versioned JSON Lines client protocol over a Unix domain socket.
Direct Hyprland integration remains isolated behind an internal backend
interface. These are Window Management decisions, not requirements for other
CLEA components; see the ADR for rationale, consequences, and deferred details.

## State invariants

Window-management behavior must preserve user-visible state across ordinary
desktop transitions. In particular, a fullscreen application must not lose
fullscreen solely because the desktop changes a common mode. Feature-specific
documentation and tests must define the transitions covered before such
behavior is implemented.

## Replaceability

Major components communicate through explicit contracts and must remain
replaceable independently. A replacement shell must not require rewriting
window-management policy, and a replacement Hyprland backend must not require
presentation clients to understand compositor-specific commands.

## Upstream strategy

Caelestia is the primary shell upstream. CLEA should retain a clear relationship
to it rather than combining multiple shells wholesale. Midnight Shell is a
secondary source of ideas and selectively portable features only; each adopted
feature requires its own architectural review, documentation, and tests.

## Deliberately open decisions

Unless narrowed by a component-specific ADR, the following choices remain
deferred until requirements justify them:

- implementation languages for components other than the Window Management
  daemon;
- IPC transports and process topology for services other than Window Management;
- the Rust async runtime, serialization framework, complete protocol schema,
  socket path, and persistence mechanism for `clea-windowd`;
- startup and supervision mechanisms, including any systemd user service;
- whether D-Bus will be added as a future Window Management adapter or used by
  other services;
- configuration schema and serialization technologies outside decisions already
  scoped to a specific component;
- packaging targets and distribution-specific integration;
- the exact feature set to retain from Caelestia or selectively port from
  Midnight Shell.
