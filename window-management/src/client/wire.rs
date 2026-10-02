//! Client side of the protocol v1 wire format: request serialization and
//! response parsing.
//!
//! The daemon owns the protocol; this module only writes the request shapes of
//! the specification and reads the response shapes back. It validates the
//! envelope and the result shape expected for the request's method, ignores
//! response fields it does not know (as the specification asks clients to do),
//! and never interprets an opaque identity.

use std::fmt;

use serde::Serialize;
use serde_json::Value;

use crate::protocol::{
    AppliedWorkspaceModeResult, ModeResult, PROTOCOL_VERSION, ProtocolError, ProtocolMode,
    ProtocolResult, RequestMethod, StatusResult, WindowsResult, WorkspaceModeResult,
    WorkspacesResult,
};

#[derive(Serialize)]
struct Envelope<'a> {
    version: u64,
    id: &'a str,
    method: &'static str,
    params: Params<'a>,
}

/// The exact parameter shapes of the specification, one per method family.
#[derive(Serialize)]
#[serde(untagged)]
enum Params<'a> {
    None {},
    Mode {
        mode: ProtocolMode,
    },
    Workspace {
        workspace_id: &'a str,
    },
    WorkspaceMode {
        workspace_id: &'a str,
        mode: ProtocolMode,
    },
}

/// Serializes one request as a single JSON line, without the terminating LF.
///
/// Fields appear in the order of the specification examples: `version`, `id`,
/// `method`, `params`.
pub fn serialize_request(id: &str, method: &RequestMethod) -> String {
    let (name, params) = match method {
        RequestMethod::Status => ("status", Params::None {}),
        RequestMethod::GetDefaultMode => ("get_default_mode", Params::None {}),
        RequestMethod::SetDefaultMode { mode } => {
            ("set_default_mode", Params::Mode { mode: *mode })
        }
        RequestMethod::ListWorkspaces => ("list_workspaces", Params::None {}),
        RequestMethod::GetWorkspaceMode { workspace_id } => (
            "get_workspace_mode",
            Params::Workspace {
                workspace_id: workspace_id.as_str(),
            },
        ),
        RequestMethod::SetWorkspaceMode { workspace_id, mode } => (
            "set_workspace_mode",
            Params::WorkspaceMode {
                workspace_id: workspace_id.as_str(),
                mode: *mode,
            },
        ),
        RequestMethod::ClearWorkspaceMode { workspace_id } => (
            "clear_workspace_mode",
            Params::Workspace {
                workspace_id: workspace_id.as_str(),
            },
        ),
        RequestMethod::ApplyWorkspaceMode { workspace_id } => (
            "apply_workspace_mode",
            Params::Workspace {
                workspace_id: workspace_id.as_str(),
            },
        ),
        RequestMethod::ListWindows => ("list_windows", Params::None {}),
    };

    serde_json::to_string(&Envelope {
        version: PROTOCOL_VERSION,
        id,
        method: name,
        params,
    })
    .expect("closed request types contain only infallibly serializable JSON values")
}

/// A well-formed response: the method's result, or the error the daemon sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Success(ProtocolResult),
    Failure(ProtocolError),
}

/// Why a response line is not a valid protocol v1 response to the request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseError {
    /// The line is not valid UTF-8 JSON.
    InvalidJson,
    /// The JSON value is not an object.
    NotAnObject,
    /// A required field is absent.
    MissingField(&'static str),
    /// A required field has the wrong JSON type or an unknown value.
    WrongFieldType(&'static str),
    /// `version` is an integer other than 1.
    UnsupportedVersion,
    /// `id` is a string other than the one that was sent.
    IdMismatch,
    /// `result` does not have the shape of the request's method.
    UnexpectedResult,
}

impl fmt::Display for ResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson => formatter.write_str("the response is not valid JSON"),
            Self::NotAnObject => formatter.write_str("the response is not a JSON object"),
            Self::MissingField(name) => {
                write!(formatter, "the response has no `{name}` field")
            }
            Self::WrongFieldType(name) => {
                write!(formatter, "the response field `{name}` has the wrong type")
            }
            Self::UnsupportedVersion => {
                formatter.write_str("the response is not protocol version 1")
            }
            Self::IdMismatch => formatter.write_str("the response id is not the request id"),
            Self::UnexpectedResult => {
                formatter.write_str("the result does not have the shape of the requested method")
            }
        }
    }
}

impl std::error::Error for ResponseError {}

/// Parses one response line (without its LF) to the request `method` that was
/// sent with correlation `id`.
///
/// The result shape is selected by the method of the request, not by guessing
/// from the response. Checks run in this order: JSON, object, `version`, `id`,
/// `ok`, then `result` or `error`.
pub fn parse_response(
    line: &[u8],
    id: &str,
    method: &RequestMethod,
) -> Result<Reply, ResponseError> {
    let Value::Object(mut object) =
        serde_json::from_slice::<Value>(line).map_err(|_| ResponseError::InvalidJson)?
    else {
        return Err(ResponseError::NotAnObject);
    };

    let version = object
        .get("version")
        .ok_or(ResponseError::MissingField("version"))?;
    let version = version
        .as_number()
        .filter(|number| number.is_i64() || number.is_u64())
        .ok_or(ResponseError::WrongFieldType("version"))?;
    if version.as_u64() != Some(PROTOCOL_VERSION) {
        return Err(ResponseError::UnsupportedVersion);
    }

    let echoed = object.get("id").ok_or(ResponseError::MissingField("id"))?;
    let echoed = echoed.as_str().ok_or(ResponseError::WrongFieldType("id"))?;
    if echoed != id {
        return Err(ResponseError::IdMismatch);
    }

    let ok = object.get("ok").ok_or(ResponseError::MissingField("ok"))?;
    let ok = ok.as_bool().ok_or(ResponseError::WrongFieldType("ok"))?;

    if ok {
        let result = object
            .remove("result")
            .ok_or(ResponseError::MissingField("result"))?;
        result_for(method, result).map(Reply::Success)
    } else {
        let error = object
            .remove("error")
            .ok_or(ResponseError::MissingField("error"))?;
        serde_json::from_value::<ProtocolError>(error)
            .map(Reply::Failure)
            .map_err(|_| ResponseError::WrongFieldType("error"))
    }
}

fn result_for(method: &RequestMethod, result: Value) -> Result<ProtocolResult, ResponseError> {
    fn shaped<T: serde::de::DeserializeOwned>(
        result: Value,
        wrap: fn(T) -> ProtocolResult,
    ) -> Result<ProtocolResult, ResponseError> {
        serde_json::from_value(result)
            .map(wrap)
            .map_err(|_| ResponseError::UnexpectedResult)
    }

    match method {
        RequestMethod::Status => shaped::<StatusResult>(result, ProtocolResult::Status),
        RequestMethod::GetDefaultMode | RequestMethod::SetDefaultMode { .. } => {
            shaped::<ModeResult>(result, ProtocolResult::Mode)
        }
        RequestMethod::ListWorkspaces => {
            shaped::<WorkspacesResult>(result, ProtocolResult::Workspaces)
        }
        RequestMethod::GetWorkspaceMode { .. }
        | RequestMethod::SetWorkspaceMode { .. }
        | RequestMethod::ClearWorkspaceMode { .. } => {
            shaped::<WorkspaceModeResult>(result, ProtocolResult::WorkspaceMode)
        }
        RequestMethod::ApplyWorkspaceMode { .. } => {
            shaped::<AppliedWorkspaceModeResult>(result, ProtocolResult::AppliedWorkspaceMode)
        }
        RequestMethod::ListWindows => shaped::<WindowsResult>(result, ProtocolResult::Windows),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::backend::fake::FakeBackend;
    use crate::core::{WindowManager, WindowPlacement, WorkspaceMode};
    use crate::protocol::{
        ErrorCode, ProtocolPlacement, ProtocolResponse, parse_request, serialize_response,
    };
    use crate::service::WindowManagementService;

    fn workspace(token: &str) -> String {
        token.to_owned()
    }

    /// The nine methods with the request lines of the protocol specification.
    /// The first six lines are the complete JSON Lines examples of the
    /// specification; the others follow the parameter shapes of its method
    /// sections.
    fn specification_cases() -> Vec<(RequestMethod, &'static str)> {
        vec![
            (
                RequestMethod::Status,
                r#"{"version":1,"id":"req-1","method":"status","params":{}}"#,
            ),
            (
                RequestMethod::GetDefaultMode,
                r#"{"version":1,"id":"req-1","method":"get_default_mode","params":{}}"#,
            ),
            (
                RequestMethod::SetDefaultMode {
                    mode: ProtocolMode::Windows,
                },
                r#"{"version":1,"id":"req-1","method":"set_default_mode","params":{"mode":"windows"}}"#,
            ),
            (
                RequestMethod::ListWorkspaces,
                r#"{"version":1,"id":"req-1","method":"list_workspaces","params":{}}"#,
            ),
            (
                RequestMethod::GetWorkspaceMode {
                    workspace_id: workspace("w:missing"),
                },
                r#"{"version":1,"id":"req-1","method":"get_workspace_mode","params":{"workspace_id":"w:missing"}}"#,
            ),
            (
                RequestMethod::SetWorkspaceMode {
                    workspace_id: workspace("w:2"),
                    mode: ProtocolMode::Windows,
                },
                r#"{"version":1,"id":"req-1","method":"set_workspace_mode","params":{"workspace_id":"w:2","mode":"windows"}}"#,
            ),
            (
                RequestMethod::ClearWorkspaceMode {
                    workspace_id: workspace("w:2"),
                },
                r#"{"version":1,"id":"req-1","method":"clear_workspace_mode","params":{"workspace_id":"w:2"}}"#,
            ),
            (
                RequestMethod::ApplyWorkspaceMode {
                    workspace_id: workspace("w:2"),
                },
                r#"{"version":1,"id":"req-1","method":"apply_workspace_mode","params":{"workspace_id":"w:2"}}"#,
            ),
            (
                RequestMethod::ListWindows,
                r#"{"version":1,"id":"req-1","method":"list_windows","params":{}}"#,
            ),
        ]
    }

    #[test]
    fn every_request_matches_the_specification_line_exactly() {
        for (method, expected) in specification_cases() {
            assert_eq!(serialize_request("req-1", &method), expected);
        }
    }

    #[test]
    fn every_request_round_trips_through_the_server_parser() {
        let cases = specification_cases();
        assert_eq!(cases.len(), 9);

        for (method, _) in cases {
            let line = serialize_request("round-trip", &method);
            assert!(!line.contains('\n'), "the line carries no LF of its own");

            let parsed = parse_request(&line).expect("the server parser should accept the line");
            assert_eq!(parsed.id(), "round-trip");
            assert_eq!(parsed.method(), &method);
        }
    }

    #[test]
    fn the_request_id_is_echoed_verbatim_and_escaped_as_json() {
        let line = serialize_request("a\"b\\c\n", &RequestMethod::Status);
        let parsed = parse_request(&line).expect("escaped id should parse");

        assert_eq!(parsed.id(), "a\"b\\c\n");
        assert!(!line.contains('\n'));
    }

    fn line_for(response: &ProtocolResponse) -> Vec<u8> {
        let mut line = serialize_response(response)
            .expect("response should serialize")
            .into_bytes();
        assert_eq!(line.pop(), Some(b'\n'));
        line
    }

    #[test]
    fn every_success_shape_the_server_serializes_is_parsed_back() {
        use crate::protocol::{WindowResult, WorkspaceResult};

        let results = [
            (
                RequestMethod::Status,
                ProtocolResult::Status(StatusResult {
                    protocol_version: 1,
                    service: "nycti-windowd".into(),
                }),
            ),
            (
                RequestMethod::GetDefaultMode,
                ProtocolResult::Mode(ModeResult {
                    mode: ProtocolMode::Tiling,
                }),
            ),
            (
                RequestMethod::SetDefaultMode {
                    mode: ProtocolMode::Windows,
                },
                ProtocolResult::Mode(ModeResult {
                    mode: ProtocolMode::Windows,
                }),
            ),
            (
                RequestMethod::ListWorkspaces,
                ProtocolResult::Workspaces(WorkspacesResult {
                    workspaces: vec![
                        WorkspaceResult {
                            workspace_id: "w:1".into(),
                            present: true,
                            default_mode: ProtocolMode::Tiling,
                            explicit_mode: None,
                            effective_mode: ProtocolMode::Tiling,
                        },
                        WorkspaceResult {
                            workspace_id: "w:2".into(),
                            present: false,
                            default_mode: ProtocolMode::Tiling,
                            explicit_mode: Some(ProtocolMode::Windows),
                            effective_mode: ProtocolMode::Windows,
                        },
                    ],
                }),
            ),
            (
                RequestMethod::GetWorkspaceMode {
                    workspace_id: "w:2".into(),
                },
                ProtocolResult::WorkspaceMode(WorkspaceModeResult {
                    workspace_id: "w:2".into(),
                    default_mode: ProtocolMode::Tiling,
                    explicit_mode: Some(ProtocolMode::Windows),
                    effective_mode: ProtocolMode::Windows,
                }),
            ),
            (
                RequestMethod::ApplyWorkspaceMode {
                    workspace_id: "w:2".into(),
                },
                ProtocolResult::AppliedWorkspaceMode(AppliedWorkspaceModeResult {
                    workspace_id: "w:2".into(),
                    effective_mode: ProtocolMode::Windows,
                }),
            ),
            (
                RequestMethod::ListWindows,
                ProtocolResult::Windows(WindowsResult {
                    windows: vec![WindowResult {
                        window_id: "win:7".into(),
                        workspace_id: "w:2".into(),
                        placement: ProtocolPlacement::Floating,
                        fullscreen: false,
                        focused: true,
                    }],
                }),
            ),
        ];

        for (method, result) in results {
            let response = ProtocolResponse::success("1".into(), result.clone());
            let parsed = parse_response(&line_for(&response), "1", &method);

            assert_eq!(parsed, Ok(Reply::Success(result)), "for {method:?}");
        }
    }

    #[test]
    fn every_method_is_answered_by_the_real_service_in_a_shape_the_client_reads() {
        let mut backend = FakeBackend::new();
        let workspace_id = backend.add_workspace(true);
        backend
            .add_window(workspace_id, WindowPlacement::Tiled, false, true)
            .expect("known workspace should accept window");
        let mut service =
            WindowManagementService::new(WindowManager::new(backend, WorkspaceMode::Tiling));

        let mut ask = |method: &RequestMethod| {
            let line = serialize_request("1", method);
            let response = service
                .handle_json_line(&line)
                .expect("response should serialize");
            parse_response(response.trim_end().as_bytes(), "1", method)
        };

        let Ok(Reply::Success(ProtocolResult::Workspaces(workspaces))) =
            ask(&RequestMethod::ListWorkspaces)
        else {
            panic!("list_workspaces should succeed");
        };
        let token = workspaces.workspaces[0].workspace_id.clone();

        let methods = [
            RequestMethod::Status,
            RequestMethod::GetDefaultMode,
            RequestMethod::SetDefaultMode {
                mode: ProtocolMode::Windows,
            },
            RequestMethod::ListWorkspaces,
            RequestMethod::GetWorkspaceMode {
                workspace_id: token.clone(),
            },
            RequestMethod::SetWorkspaceMode {
                workspace_id: token.clone(),
                mode: ProtocolMode::Windows,
            },
            RequestMethod::ClearWorkspaceMode {
                workspace_id: token.clone(),
            },
            RequestMethod::ApplyWorkspaceMode {
                workspace_id: token.clone(),
            },
            RequestMethod::ListWindows,
        ];
        for method in methods {
            let reply = ask(&method);
            assert!(
                matches!(reply, Ok(Reply::Success(_))),
                "{method:?} gave {reply:?}"
            );
        }
    }

    #[test]
    fn every_error_code_is_parsed_back_with_its_message() {
        let codes = [
            ErrorCode::InvalidRequest,
            ErrorCode::UnsupportedVersion,
            ErrorCode::UnknownMethod,
            ErrorCode::InvalidParams,
            ErrorCode::UnknownWorkspace,
            ErrorCode::UnknownWindow,
            ErrorCode::CompositorUnavailable,
            ErrorCode::ActionFailed,
            ErrorCode::ObservedStateUnavailable,
            ErrorCode::InconsistentObservedState,
            ErrorCode::InternalError,
        ];

        for code in codes {
            let response =
                ProtocolResponse::error(Some("1".into()), code, format!("message for {code:?}"));
            let parsed = parse_response(&line_for(&response), "1", &RequestMethod::ListWindows);

            let Ok(Reply::Failure(error)) = parsed else {
                panic!("{code:?} should parse as a failure, got {parsed:?}");
            };
            assert_eq!(error.code(), code);
            assert_eq!(error.message(), format!("message for {code:?}"));
        }
    }

    fn parse(line: &str) -> Result<Reply, ResponseError> {
        parse_response(line.as_bytes(), "1", &RequestMethod::ListWorkspaces)
    }

    #[test]
    fn each_malformed_response_has_its_own_error() {
        let ok_result = |result: serde_json::Value| {
            json!({"version": 1, "id": "1", "ok": true, "result": result}).to_string()
        };
        let cases: Vec<(&str, String, ResponseError)> = vec![
            ("empty line", String::new(), ResponseError::InvalidJson),
            ("invalid JSON", "{".into(), ResponseError::InvalidJson),
            ("array", "[]".into(), ResponseError::NotAnObject),
            ("scalar", "1".into(), ResponseError::NotAnObject),
            ("null", "null".into(), ResponseError::NotAnObject),
            (
                "no version",
                r#"{"id":"1","ok":true,"result":{}}"#.into(),
                ResponseError::MissingField("version"),
            ),
            (
                "text version",
                r#"{"version":"1","id":"1","ok":true,"result":{}}"#.into(),
                ResponseError::WrongFieldType("version"),
            ),
            (
                "fractional version",
                r#"{"version":1.5,"id":"1","ok":true,"result":{}}"#.into(),
                ResponseError::WrongFieldType("version"),
            ),
            (
                "version 2",
                r#"{"version":2,"id":"1","ok":true,"result":{}}"#.into(),
                ResponseError::UnsupportedVersion,
            ),
            (
                "version 0",
                r#"{"version":0,"id":"1","ok":true,"result":{}}"#.into(),
                ResponseError::UnsupportedVersion,
            ),
            (
                "no id",
                r#"{"version":1,"ok":true,"result":{}}"#.into(),
                ResponseError::MissingField("id"),
            ),
            (
                "numeric id",
                r#"{"version":1,"id":1,"ok":true,"result":{}}"#.into(),
                ResponseError::WrongFieldType("id"),
            ),
            (
                "null id",
                r#"{"version":1,"id":null,"ok":false,"error":{"code":"invalid_request","message":"m"}}"#
                    .into(),
                ResponseError::WrongFieldType("id"),
            ),
            (
                "another id",
                r#"{"version":1,"id":"2","ok":true,"result":{}}"#.into(),
                ResponseError::IdMismatch,
            ),
            (
                "no ok",
                r#"{"version":1,"id":"1","result":{}}"#.into(),
                ResponseError::MissingField("ok"),
            ),
            (
                "text ok",
                r#"{"version":1,"id":"1","ok":"true","result":{}}"#.into(),
                ResponseError::WrongFieldType("ok"),
            ),
            (
                "success without result",
                r#"{"version":1,"id":"1","ok":true}"#.into(),
                ResponseError::MissingField("result"),
            ),
            (
                "failure without error",
                r#"{"version":1,"id":"1","ok":false}"#.into(),
                ResponseError::MissingField("error"),
            ),
            (
                "error that is not an object",
                r#"{"version":1,"id":"1","ok":false,"error":"boom"}"#.into(),
                ResponseError::WrongFieldType("error"),
            ),
            (
                "error without message",
                r#"{"version":1,"id":"1","ok":false,"error":{"code":"internal_error"}}"#.into(),
                ResponseError::WrongFieldType("error"),
            ),
            (
                "error with an unknown code",
                r#"{"version":1,"id":"1","ok":false,"error":{"code":"teapot","message":"m"}}"#
                    .into(),
                ResponseError::WrongFieldType("error"),
            ),
            (
                "result of another method",
                ok_result(json!({"mode": "tiling"})),
                ResponseError::UnexpectedResult,
            ),
            (
                "result that is not an object",
                ok_result(json!([])),
                ResponseError::UnexpectedResult,
            ),
            (
                "result field of the wrong type",
                ok_result(json!({"workspaces": [{
                    "workspace_id": "w:1", "present": "yes", "default_mode": "tiling",
                    "explicit_mode": null, "effective_mode": "tiling"
                }]})),
                ResponseError::UnexpectedResult,
            ),
            (
                "result field missing",
                ok_result(json!({"workspaces": [{
                    "workspace_id": "w:1", "present": true, "default_mode": "tiling",
                    "explicit_mode": null
                }]})),
                ResponseError::UnexpectedResult,
            ),
            (
                "unknown mode value",
                ok_result(json!({"workspaces": [{
                    "workspace_id": "w:1", "present": true, "default_mode": "floating",
                    "explicit_mode": null, "effective_mode": "tiling"
                }]})),
                ResponseError::UnexpectedResult,
            ),
        ];

        for (name, line, expected) in cases {
            assert_eq!(parse(&line), Err(expected), "case: {name}");
        }
    }

    #[test]
    fn a_response_that_is_not_utf8_is_invalid_json() {
        let parsed = parse_response(b"{\"version\":\xff}", "1", &RequestMethod::Status);

        assert_eq!(parsed, Err(ResponseError::InvalidJson));
    }

    #[test]
    fn the_result_shape_follows_the_request_method() {
        let line = r#"{"version":1,"id":"1","ok":true,"result":{"mode":"windows"}}"#.as_bytes();

        assert!(matches!(
            parse_response(line, "1", &RequestMethod::GetDefaultMode),
            Ok(Reply::Success(ProtocolResult::Mode(_)))
        ));
        assert_eq!(
            parse_response(line, "1", &RequestMethod::ListWindows),
            Err(ResponseError::UnexpectedResult)
        );
    }

    #[test]
    fn unknown_response_fields_are_ignored() {
        let line = json!({
            "version": 1, "id": "1", "ok": true, "future": [1, 2],
            "result": {"workspaces": [{
                "workspace_id": "w:1", "present": true, "default_mode": "tiling",
                "explicit_mode": null, "effective_mode": "tiling", "monitor": "ignored"
            }], "extra": null}
        })
        .to_string();

        let parsed = parse(&line);

        let Ok(Reply::Success(ProtocolResult::Workspaces(result))) = parsed else {
            panic!("unknown fields should be ignored, got {parsed:?}");
        };
        assert_eq!(result.workspaces.len(), 1);
    }

    #[test]
    fn an_error_response_may_carry_unknown_fields_too() {
        let line = r#"{"version":1,"id":"1","ok":false,"extra":true,"error":{"code":"unknown_workspace","message":"m","hint":"x"}}"#;

        let parsed = parse(line);

        let Ok(Reply::Failure(error)) = parsed else {
            panic!("expected a failure, got {parsed:?}");
        };
        assert_eq!(error.code(), ErrorCode::UnknownWorkspace);
    }
}
