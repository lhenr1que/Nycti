//! Daemon execution pieces for CLEA Window Management.
//!
//! Currently this contains the single service authority and the per-connection
//! workers described by
//! [ADR 0006](../../../docs/architecture/adr/0006-window-management-daemon-execution.md).
//! The accept loop and the coordinator that starts them are not implemented.

mod authority;
#[cfg(test)]
mod test_support;
mod worker;

pub use authority::{Authority, AuthorityClient, AuthorityError, AuthorityLifecycleError};
pub use worker::{WorkerExit, WorkerHandle, WorkerLifecycleError, spawn_worker};
