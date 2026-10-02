use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::core::{WindowPlacement, WorkspaceMode};

use super::PROTOCOL_VERSION;

/// A validated protocol v1 request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolRequest {
    pub(crate) id: String,
    pub(crate) method: RequestMethod,
}

impl ProtocolRequest {
    /// Returns the client-selected correlation ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the validated operation.
    pub const fn method(&self) -> &RequestMethod {
        &self.method
    }
}

/// The closed set of protocol v1 operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestMethod {
    Status,
    GetDefaultMode,
    SetDefaultMode {
        mode: ProtocolMode,
    },
    ListWorkspaces,
    GetWorkspaceMode {
        workspace_id: String,
    },
    SetWorkspaceMode {
        workspace_id: String,
        mode: ProtocolMode,
    },
    ClearWorkspaceMode {
        workspace_id: String,
    },
    ApplyWorkspaceMode {
        workspace_id: String,
    },
    ListWindows,
}

/// A workspace mode as represented on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolMode {
    Tiling,
    Windows,
}

impl From<ProtocolMode> for WorkspaceMode {
    fn from(mode: ProtocolMode) -> Self {
        match mode {
            ProtocolMode::Tiling => Self::Tiling,
            ProtocolMode::Windows => Self::Windows,
        }
    }
}

impl From<WorkspaceMode> for ProtocolMode {
    fn from(mode: WorkspaceMode) -> Self {
        match mode {
            WorkspaceMode::Tiling => Self::Tiling,
            WorkspaceMode::Windows => Self::Windows,
        }
    }
}

/// A window placement as represented on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolPlacement {
    Tiled,
    Floating,
}

impl From<WindowPlacement> for ProtocolPlacement {
    fn from(placement: WindowPlacement) -> Self {
        match placement {
            WindowPlacement::Tiled => Self::Tiled,
            WindowPlacement::Floating => Self::Floating,
        }
    }
}

/// The stable protocol error vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    UnsupportedVersion,
    UnknownMethod,
    InvalidParams,
    UnknownWorkspace,
    UnknownWindow,
    CompositorUnavailable,
    ActionFailed,
    ObservedStateUnavailable,
    InconsistentObservedState,
    InternalError,
}

/// A compositor-independent public protocol error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    code: ErrorCode,
    message: String,
}

impl ProtocolError {
    pub(crate) fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Returns the stable machine-readable error code.
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns the non-stable human-readable message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// A protocol v1 response with mutually exclusive success and error shapes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ProtocolResponse {
    Success(SuccessResponse),
    Error(ErrorResponse),
}

impl ProtocolResponse {
    pub(crate) fn success(id: String, result: ProtocolResult) -> Self {
        Self::Success(SuccessResponse {
            version: PROTOCOL_VERSION,
            id,
            ok: true,
            result,
        })
    }

    pub(crate) fn error(id: Option<String>, code: ErrorCode, message: impl Into<String>) -> Self {
        Self::from_protocol_error(id, ProtocolError::new(code, message))
    }

    pub(crate) fn from_protocol_error(id: Option<String>, error: ProtocolError) -> Self {
        Self::Error(ErrorResponse {
            version: PROTOCOL_VERSION,
            id,
            ok: false,
            error,
        })
    }

    /// Returns the echoed correlation ID, if a valid one was recoverable.
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Success(response) => Some(&response.id),
            Self::Error(response) => response.id.as_deref(),
        }
    }

    /// Returns the protocol error for an error response.
    pub const fn error_value(&self) -> Option<&ProtocolError> {
        match self {
            Self::Success(_) => None,
            Self::Error(response) => Some(&response.error),
        }
    }

    /// Returns whether this is a successful response.
    pub const fn is_success(&self) -> bool {
        matches!(self, Self::Success(_))
    }
}

/// Serialized success envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SuccessResponse {
    version: u64,
    id: String,
    ok: bool,
    result: ProtocolResult,
}

/// Serialized error envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ErrorResponse {
    version: u64,
    id: Option<String>,
    ok: bool,
    error: ProtocolError,
}

/// The closed set of successful protocol v1 result shapes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ProtocolResult {
    Status(StatusResult),
    Mode(ModeResult),
    Workspaces(WorkspacesResult),
    WorkspaceMode(WorkspaceModeResult),
    AppliedWorkspaceMode(AppliedWorkspaceModeResult),
    Windows(WindowsResult),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResult {
    pub(crate) protocol_version: u64,
    pub(crate) service: Cow<'static, str>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeResult {
    pub(crate) mode: ProtocolMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacesResult {
    pub(crate) workspaces: Vec<WorkspaceResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceResult {
    pub(crate) workspace_id: String,
    pub(crate) present: bool,
    pub(crate) default_mode: ProtocolMode,
    pub(crate) explicit_mode: Option<ProtocolMode>,
    pub(crate) effective_mode: ProtocolMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceModeResult {
    pub(crate) workspace_id: String,
    pub(crate) default_mode: ProtocolMode,
    pub(crate) explicit_mode: Option<ProtocolMode>,
    pub(crate) effective_mode: ProtocolMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedWorkspaceModeResult {
    pub(crate) workspace_id: String,
    pub(crate) effective_mode: ProtocolMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsResult {
    pub(crate) windows: Vec<WindowResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowResult {
    pub(crate) window_id: String,
    pub(crate) workspace_id: String,
    pub(crate) placement: ProtocolPlacement,
    pub(crate) fullscreen: bool,
    pub(crate) focused: bool,
}

/// Serializes one response as exactly one newline-terminated JSON line.
pub fn serialize_response(response: &ProtocolResponse) -> Result<String, serde_json::Error> {
    serde_json::to_string(response).map(|mut json| {
        json.push('\n');
        json
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn success_serializes_result_without_error_and_with_newline() {
        let response = ProtocolResponse::success(
            "request-1".into(),
            ProtocolResult::Mode(ModeResult {
                mode: ProtocolMode::Windows,
            }),
        );

        let line = serialize_response(&response).expect("response should serialize");
        let value: Value = serde_json::from_str(&line).expect("response should be JSON");

        assert!(line.ends_with('\n'));
        assert_eq!(value["version"], 1);
        assert_eq!(value["id"], "request-1");
        assert_eq!(value["ok"], true);
        assert_eq!(value["result"]["mode"], "windows");
        assert!(value.get("error").is_none());
    }

    #[test]
    fn error_serializes_error_without_result() {
        let response = ProtocolResponse::error(
            Some("request-1".into()),
            ErrorCode::InvalidParams,
            "invalid parameters",
        );

        let line = serialize_response(&response).expect("response should serialize");
        let value: Value = serde_json::from_str(&line).expect("response should be JSON");

        assert_eq!(
            value,
            json!({
                "version": 1,
                "id": "request-1",
                "ok": false,
                "error": {"code": "invalid_params", "message": "invalid parameters"}
            })
        );
        assert!(value.get("result").is_none());
    }

    #[test]
    fn unrecoverable_correlation_id_serializes_as_null() {
        let response = ProtocolResponse::error(None, ErrorCode::InvalidRequest, "invalid request");

        let line = serialize_response(&response).expect("response should serialize");
        let value: Value = serde_json::from_str(&line).expect("response should be JSON");

        assert_eq!(value["id"], Value::Null);
    }

    #[test]
    fn protocol_enums_use_exact_canonical_strings() {
        assert_eq!(
            serde_json::to_value(ProtocolMode::Tiling).expect("mode should serialize"),
            "tiling"
        );
        assert_eq!(
            serde_json::to_value(ProtocolMode::Windows).expect("mode should serialize"),
            "windows"
        );
        assert_eq!(
            serde_json::to_value(ProtocolPlacement::Tiled).expect("placement should serialize"),
            "tiled"
        );
        assert_eq!(
            serde_json::to_value(ProtocolPlacement::Floating).expect("placement should serialize"),
            "floating"
        );
    }

    #[test]
    fn every_error_code_uses_its_stable_wire_value() {
        let cases = [
            (ErrorCode::InvalidRequest, "invalid_request"),
            (ErrorCode::UnsupportedVersion, "unsupported_version"),
            (ErrorCode::UnknownMethod, "unknown_method"),
            (ErrorCode::InvalidParams, "invalid_params"),
            (ErrorCode::UnknownWorkspace, "unknown_workspace"),
            (ErrorCode::UnknownWindow, "unknown_window"),
            (ErrorCode::CompositorUnavailable, "compositor_unavailable"),
            (ErrorCode::ActionFailed, "action_failed"),
            (
                ErrorCode::ObservedStateUnavailable,
                "observed_state_unavailable",
            ),
            (
                ErrorCode::InconsistentObservedState,
                "inconsistent_observed_state",
            ),
            (ErrorCode::InternalError, "internal_error"),
        ];

        for (code, expected) in cases {
            assert_eq!(
                serde_json::to_value(code).expect("error code should serialize"),
                expected
            );
        }
    }
}
