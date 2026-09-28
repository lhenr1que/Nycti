//! Typed parsing and serialization for CLEA Window Management Protocol v1.

mod parser;
mod types;

pub use parser::parse_request;
pub(crate) use types::{
    AppliedWorkspaceModeResult, ModeResult, ProtocolResult, StatusResult, WindowResult,
    WindowsResult, WorkspaceModeResult, WorkspaceResult, WorkspacesResult,
};
pub use types::{
    ErrorCode, ProtocolError, ProtocolMode, ProtocolPlacement, ProtocolRequest, ProtocolResponse,
    RequestMethod, serialize_response,
};

/// The only protocol version implemented by this crate.
pub const PROTOCOL_VERSION: u64 = 1;
