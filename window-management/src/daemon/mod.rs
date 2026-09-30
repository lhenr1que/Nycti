//! Daemon execution pieces for CLEA Window Management.
//!
//! This contains the single service authority, the per-connection workers, and
//! the coordinator that runs the accept loop and the shutdown sequence. They are
//! described by
//! [ADR 0006](../../../docs/architecture/adr/0006-window-management-daemon-execution.md)
//! and
//! [ADR 0007](../../../docs/architecture/adr/0007-window-management-daemon-coordinator.md).
//! Signal handling and a functional `main.rs` are not implemented.

mod authority;
mod coordinator;
#[cfg(test)]
mod test_support;
mod worker;

pub use authority::{Authority, AuthorityClient, AuthorityError, AuthorityLifecycleError};
pub use coordinator::{
    Coordinator, CoordinatorError, CoordinatorReport, ShutdownError, ShutdownHandle, StopReason,
};
pub use worker::{WorkerExit, WorkerHandle, WorkerLifecycleError, spawn_worker};
