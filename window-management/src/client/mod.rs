//! Client for Nycti Window Management protocol v1.
//!
//! The client is never an authority (ADR 0002): it sends typed requests to
//! `nycti-windowd` through its Unix socket and shows what the daemon answers.
//! It owns no policy, never reaches the compositor, and never builds or infers
//! an opaque identity token. The design is recorded in
//! [ADR 0010](../../../docs/architecture/adr/0010-window-management-cli.md).
//!
//! - [`wire`] serializes requests and parses responses.

pub mod wire;
