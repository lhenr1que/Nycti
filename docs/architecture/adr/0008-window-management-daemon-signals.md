# ADR 0008: Window Management Daemon Signals and Process Lifecycle

- Status: Accepted
- Date: 2026-09-30

## Context

[ADR 0006](0006-window-management-daemon-execution.md) leaves graceful
`SIGTERM`/`SIGINT` handling and supervision as future work, and lists `ctrlc`,
`signal-hook`, and direct `libc` signal handling among the things its initial
boundary does not select. [ADR 0007](0007-window-management-daemon-coordinator.md)
provides `ShutdownHandle`, which connects to the daemon's own socket and is
therefore not safe to call from a signal handler. The standard library has no
signal API. A functional `main.rs` needs one.

ADR 0006 restricts an asynchronous runtime and channel or runtime libraries
(for example Tokio and crossbeam). It does not forbid dependencies in general,
and the crate already depends on `serde` and `serde_json`.

This ADR records the choices made for signals and the process lifecycle. It does
not modify ADR 0006 or ADR 0007.

## Decision

### Signal dependency

`clea-windowd` adds one dependency for signals: `signal-hook`, with the
`iterator` feature only (`default-features = false`; the `iterator` feature
requires the empty `channel` feature). It brings `signal-hook-registry` and
`libc`. The project's own code stays free of `unsafe`.

Licenses, read from the crate manifests and license files of the packages in the
local crates.io registry copy used for the lockfile:

| Package | Version | License |
| --- | --- | --- |
| `signal-hook` | 0.3.18 | Apache-2.0 / MIT |
| `signal-hook-registry` | 1.4.5 | Apache-2.0 / MIT |
| `libc` | 0.2.174 | MIT OR Apache-2.0 |

All three are offered under MIT, and under Apache-2.0 as an alternative. Both
licenses are compatible with the GPL-3.0 of this project (code under either can
be combined into a GPL-3.0 work, keeping its notices). The licenses of the
dependencies of the project were not checked against any source other than those
manifests and license files.

### Signal thread

Signals are registered before the listener is bound, so that a signal that
arrives during startup stays pending instead of killing the process and leaving a
stale socket. A dedicated thread waits for `SIGTERM` or `SIGINT` and then calls
`ShutdownHandle::request_shutdown`. No signal handler calls it.

The first signal requests an orderly shutdown. A second signal forces an
immediate process exit with code 7, without running destructors, so the socket
file stays on disk and the next start recovers it as a stale socket. Signals of
the same kind that arrive before the thread reads them can be merged, so the
thread counts the signals it reads, not the signals that were sent.

If the signal source ends, the daemon shuts down, because a daemon that can no
longer be stopped by a signal should not keep running.

The thread writes `shutdown requested` and the forced-exit message directly to
its own writer, which is standard error in the binary, and not through the main
thread's writer. When the coordinator stops, the main thread closes the source
and joins the signal thread. The thread is not left running.

`SIGHUP` keeps its default action (provisional): closing the terminal ends the
daemon without cleanup, and the next start recovers the stale socket. `SIGPIPE`
is ignored by the Rust runtime for binaries, which the daemon relies on, since a
write to a closed connection must be an error and not a signal.

### Where the code lives

The logic lives in the library behind a `ShutdownSource` trait, so that tests
inject a source and never install a real handler in the test process. Only the
binary knows `signal-hook`. The library exposes `run_from_env` (startup steps 1
to 3 of ADR 0006, then the coordinator) and `run_with` (listener and service
already built).

### Startup and failure

The startup order is the one of ADR 0006: bind the listener, construct the
`HyprlandBackend`, the manager, and the service, then start the coordinator.
Registering the signals comes first. There is no automatic retry. If a step
fails, a message goes to standard error and the listener, if it was bound, is
dropped, which removes its socket by recorded device and inode. A stale socket is
recovered by the bind itself. An active daemon causes `AlreadyRunning`.

The daemon does not probe the compositor at startup. `HyprlandBackend::from_env`
only resolves the socket path from `XDG_RUNTIME_DIR` and
`HYPRLAND_INSTANCE_SIGNATURE`, and fails only when they are missing or invalid.
A stopped compositor shows up as `compositor_unavailable` on each request that
needs it, while `status` keeps answering.

### Exit codes (provisional)

| Code | Meaning |
| --- | --- |
| 0 | shutdown requested and every shutdown step ended cleanly |
| 1 | startup failure |
| 2 | already running |
| 3 | authority failed |
| 4 | accept failed |
| 5 | the coordinator panicked |
| 6 | shutdown requested, but a shutdown step failed |
| 7 | forced exit by a second signal |

The codes are provisional and promise no stability.

### Diagnostics

There is no logging framework. The daemon writes plain text lines to standard
error, each starting with `clea-windowd:`: the listening socket path, the
shutdown request, startup errors, and a final report with the stop reason, the
worker counts, and the result of the authority and listener cleanup. The text
promises no stable format.

The process-level tests wait for the `listening` and `shutdown requested` lines
to synchronize with the daemon. This ties the tests to provisional text.

## Alternatives considered

`libc` or `nix` with `sigwait` in a dedicated thread: needs signals blocked in
every thread before any thread starts, and either `unsafe` code or a heavier
dependency. `ctrlc`: one global handler and fewer signals. `signalfd`: specific to
Linux and has the same masking requirement. Polling an `AtomicBool`: polling, which
ADR 0007 rejected for the accept loop. `tokio::signal`: an asynchronous runtime,
excluded by ADR 0006.

## Consequences

- One new dependency and a lockfile change of three packages.
- The signal logic is tested with an injected source; the real `signal-hook`
  source is exercised by process-level tests that signal a child process.
- ADR 0007 stays Proposed until this milestone has validated `ShutdownHandle`
  with real signals.

### Debts and limitations (provisional)

- The daemon is bound to the Hyprland instance resolved at startup
  (`HYPRLAND_INSTANCE_SIGNATURE`). If Hyprland restarts with another signature,
  the daemon must be restarted.
- If the signal thread dies (for example by panicking) before the daemon stops,
  nothing reads the signals any more. Once `signal-hook` has installed its
  handling, `SIGTERM` and `SIGINT` are then ignored instead of using the system
  default action, and only `SIGKILL` ends the daemon. The thread is joined only
  after the coordinator stops, and its result is ignored, so this failure is not
  observed. This is provisional.
- The default mode `Tiling` is fixed in `main`, with no configuration.
- The process-level tests depend on `/usr/bin/kill` to signal the child process.
  This is a dependency of the test environment.
- A forced exit leaves a stale socket on disk.
- No logging framework, no `SIGHUP` policy, no connection limits, and no startup
  probe of the compositor.
- Exit codes and standard error text are provisional.

## Out of scope

systemd integration, daemonization, reconfiguration on a signal, a protocol
shutdown method, and any change to ADR 0006 or ADR 0007.
