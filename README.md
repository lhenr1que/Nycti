# CLEA Desktop

CLEA is a modular desktop environment for Hyprland. Its shell is built with
Caelestia as the primary upstream, while keeping system policy outside the
presentation layer.

The project is divided into independently replaceable components for the shell,
settings, window management, services, configuration, packaging, and tests.
Shell, Settings, and CLI clients communicate with system capabilities through
documented APIs or IPC boundaries. Direct Hyprland integration is isolated
behind dedicated interfaces and backends.

See the [architecture overview](docs/architecture/overview.md),
[architectural principles](docs/architecture/principles.md), and
[architecture decision records](docs/architecture/adr/) for the project
boundaries and decisions established so far.
