//! Daemon execution pieces for CLEA Window Management.
//!
//! Currently this contains the single service authority described by
//! [ADR 0006](../../../docs/architecture/adr/0006-window-management-daemon-execution.md).
//! The accept loop, connection workers, and coordinator are not implemented.

mod authority;
#[cfg(test)]
mod test_support;

pub use authority::{Authority, AuthorityClient, AuthorityError, AuthorityLifecycleError};
