use serde_json::{Map, Value};

use super::PROTOCOL_VERSION;
use super::types::{ErrorCode, ProtocolMode, ProtocolRequest, ProtocolResponse, RequestMethod};

const ENVELOPE_FIELDS: [&str; 4] = ["version", "id", "method", "params"];

/// Parses and validates one complete protocol request line.
///
/// A trailing newline is accepted as JSON whitespace. Framing multiple lines is
/// deliberately outside this function.
pub fn parse_request(line: &str) -> Result<ProtocolRequest, ProtocolResponse> {
    let value = serde_json::from_str::<Value>(line)
        .map_err(|_| invalid_request(None, "request line must contain one valid JSON value"))?;
    let object = value
        .as_object()
        .ok_or_else(|| invalid_request(None, "request must be a top-level JSON object"))?;

    let id = valid_correlation_id(object.get("id")).map(str::to_owned);
    let id = id.ok_or_else(|| invalid_request(None, "id must be a non-empty string"))?;

    let version = object
        .get("version")
        .ok_or_else(|| invalid_request(Some(id.clone()), "version is required"))?;
    if !is_json_integer(version) {
        return Err(invalid_request(Some(id), "version must be a JSON integer"));
    }

    let method = object
        .get("method")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let method = method
        .ok_or_else(|| invalid_request(Some(id.clone()), "method must be a non-empty string"))?;

    if !object.contains_key("params") {
        return Err(invalid_request(Some(id), "params is required"));
    }

    if !is_version_one(version) {
        return Err(ProtocolResponse::error(
            Some(id),
            ErrorCode::UnsupportedVersion,
            "protocol version is not supported",
        ));
    }

    if object.len() != ENVELOPE_FIELDS.len()
        || object
            .keys()
            .any(|field| !ENVELOPE_FIELDS.contains(&field.as_str()))
    {
        return Err(invalid_request(
            Some(id),
            "version 1 requests contain unknown top-level fields",
        ));
    }

    let method_kind = KnownMethod::from_name(method).ok_or_else(|| {
        ProtocolResponse::error(
            Some(id.clone()),
            ErrorCode::UnknownMethod,
            "method is not supported",
        )
    })?;

    let params = object["params"].as_object().ok_or_else(|| {
        ProtocolResponse::error(
            Some(id.clone()),
            ErrorCode::InvalidParams,
            "params must be a JSON object",
        )
    })?;

    let method = method_kind.parse_params(params).map_err(|message| {
        ProtocolResponse::error(Some(id.clone()), ErrorCode::InvalidParams, message)
    })?;

    Ok(ProtocolRequest { id, method })
}

fn valid_correlation_id(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|id| !id.is_empty())
}

fn is_json_integer(value: &Value) -> bool {
    value
        .as_number()
        .is_some_and(|number| number.is_i64() || number.is_u64())
}

fn is_version_one(value: &Value) -> bool {
    value.as_u64() == Some(PROTOCOL_VERSION)
}

fn invalid_request(id: Option<String>, message: &'static str) -> ProtocolResponse {
    ProtocolResponse::error(id, ErrorCode::InvalidRequest, message)
}

#[derive(Clone, Copy)]
enum KnownMethod {
    Status,
    GetDefaultMode,
    SetDefaultMode,
    ListWorkspaces,
    GetWorkspaceMode,
    SetWorkspaceMode,
    ClearWorkspaceMode,
    ApplyWorkspaceMode,
    ListWindows,
}

impl KnownMethod {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "status" => Some(Self::Status),
            "get_default_mode" => Some(Self::GetDefaultMode),
            "set_default_mode" => Some(Self::SetDefaultMode),
            "list_workspaces" => Some(Self::ListWorkspaces),
            "get_workspace_mode" => Some(Self::GetWorkspaceMode),
            "set_workspace_mode" => Some(Self::SetWorkspaceMode),
            "clear_workspace_mode" => Some(Self::ClearWorkspaceMode),
            "apply_workspace_mode" => Some(Self::ApplyWorkspaceMode),
            "list_windows" => Some(Self::ListWindows),
            _ => None,
        }
    }

    fn parse_params(self, params: &Map<String, Value>) -> Result<RequestMethod, &'static str> {
        match self {
            Self::Status => {
                require_exact_fields(params, &[])?;
                Ok(RequestMethod::Status)
            }
            Self::GetDefaultMode => {
                require_exact_fields(params, &[])?;
                Ok(RequestMethod::GetDefaultMode)
            }
            Self::SetDefaultMode => {
                require_exact_fields(params, &["mode"])?;
                Ok(RequestMethod::SetDefaultMode {
                    mode: parse_mode(params.get("mode"))?,
                })
            }
            Self::ListWorkspaces => {
                require_exact_fields(params, &[])?;
                Ok(RequestMethod::ListWorkspaces)
            }
            Self::GetWorkspaceMode => {
                require_exact_fields(params, &["workspace_id"])?;
                Ok(RequestMethod::GetWorkspaceMode {
                    workspace_id: parse_workspace_id(params.get("workspace_id"))?,
                })
            }
            Self::SetWorkspaceMode => {
                require_exact_fields(params, &["workspace_id", "mode"])?;
                Ok(RequestMethod::SetWorkspaceMode {
                    workspace_id: parse_workspace_id(params.get("workspace_id"))?,
                    mode: parse_mode(params.get("mode"))?,
                })
            }
            Self::ClearWorkspaceMode => {
                require_exact_fields(params, &["workspace_id"])?;
                Ok(RequestMethod::ClearWorkspaceMode {
                    workspace_id: parse_workspace_id(params.get("workspace_id"))?,
                })
            }
            Self::ApplyWorkspaceMode => {
                require_exact_fields(params, &["workspace_id"])?;
                Ok(RequestMethod::ApplyWorkspaceMode {
                    workspace_id: parse_workspace_id(params.get("workspace_id"))?,
                })
            }
            Self::ListWindows => {
                require_exact_fields(params, &[])?;
                Ok(RequestMethod::ListWindows)
            }
        }
    }
}

fn require_exact_fields(
    params: &Map<String, Value>,
    expected: &[&str],
) -> Result<(), &'static str> {
    if params.len() != expected.len() || expected.iter().any(|field| !params.contains_key(*field)) {
        return Err("params must contain exactly the fields required by the method");
    }
    Ok(())
}

fn parse_mode(value: Option<&Value>) -> Result<ProtocolMode, &'static str> {
    match value.and_then(Value::as_str) {
        Some("tiling") => Ok(ProtocolMode::Tiling),
        Some("windows") => Ok(ProtocolMode::Windows),
        _ => Err("mode must be either tiling or windows"),
    }
}

fn parse_workspace_id(value: Option<&Value>) -> Result<String, &'static str> {
    value
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or("workspace_id must be a non-empty string")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn error_for(line: &str) -> ProtocolResponse {
        parse_request(line).expect_err("request should be rejected")
    }

    fn assert_error(line: &str, code: ErrorCode, id: Option<&str>) {
        let response = error_for(line);
        assert_eq!(response.id(), id);
        assert_eq!(response.error_value().map(ProtocolError::code), Some(code));
    }

    use crate::protocol::ProtocolError;

    #[test]
    fn parses_valid_status_with_or_without_trailing_newline() {
        let line = r#"{"version":1,"id":"request-1","method":"status","params":{}}"#;
        let request = parse_request(line).expect("status should parse");
        assert_eq!(request.id(), "request-1");
        assert_eq!(request.method(), &RequestMethod::Status);
        assert_eq!(parse_request(&format!("{line}\n")), Ok(request));
    }

    #[test]
    fn invalid_json_has_no_correlation_id() {
        assert_error("{", ErrorCode::InvalidRequest, None);
    }

    #[test]
    fn top_level_array_is_invalid_request() {
        assert_error("[]", ErrorCode::InvalidRequest, None);
    }

    #[test]
    fn missing_version_echoes_recovered_id() {
        assert_error(
            r#"{"id":"request-1","method":"status","params":{}}"#,
            ErrorCode::InvalidRequest,
            Some("request-1"),
        );
    }

    #[test]
    fn string_version_is_invalid_request() {
        assert_error(
            r#"{"version":"1","id":"request-1","method":"status","params":{}}"#,
            ErrorCode::InvalidRequest,
            Some("request-1"),
        );
    }

    #[test]
    fn missing_id_is_invalid_request() {
        assert_error(
            r#"{"version":1,"method":"status","params":{}}"#,
            ErrorCode::InvalidRequest,
            None,
        );
    }

    #[test]
    fn empty_id_is_invalid_request() {
        assert_error(
            r#"{"version":1,"id":"","method":"status","params":{}}"#,
            ErrorCode::InvalidRequest,
            None,
        );
    }

    #[test]
    fn missing_method_is_invalid_request() {
        assert_error(
            r#"{"version":1,"id":"request-1","params":{}}"#,
            ErrorCode::InvalidRequest,
            Some("request-1"),
        );
    }

    #[test]
    fn missing_params_is_invalid_request() {
        assert_error(
            r#"{"version":1,"id":"request-1","method":"status"}"#,
            ErrorCode::InvalidRequest,
            Some("request-1"),
        );
    }

    #[test]
    fn unsupported_version_precedes_v1_method_and_params_validation() {
        assert_error(
            r#"{"version":2,"id":"request-1","method":"nonsense","params":"nonsense","extra":true}"#,
            ErrorCode::UnsupportedVersion,
            Some("request-1"),
        );
    }

    #[test]
    fn unknown_v1_top_level_field_is_invalid_request() {
        assert_error(
            r#"{"version":1,"id":"request-1","method":"status","params":{},"extra":true}"#,
            ErrorCode::InvalidRequest,
            Some("request-1"),
        );
    }

    #[test]
    fn unknown_method_precedes_params_validation() {
        assert_error(
            r#"{"version":1,"id":"request-1","method":"nonsense","params":"anything"}"#,
            ErrorCode::UnknownMethod,
            Some("request-1"),
        );
    }

    #[test]
    fn known_method_with_non_object_params_is_invalid_params() {
        assert_error(
            r#"{"version":1,"id":"request-1","method":"status","params":"anything"}"#,
            ErrorCode::InvalidParams,
            Some("request-1"),
        );
    }

    #[test]
    fn missing_method_parameter_is_invalid_params() {
        assert_error(
            r#"{"version":1,"id":"request-1","method":"set_default_mode","params":{}}"#,
            ErrorCode::InvalidParams,
            Some("request-1"),
        );
    }

    #[test]
    fn extra_method_parameter_is_invalid_params() {
        assert_error(
            r#"{"version":1,"id":"request-1","method":"set_default_mode","params":{"mode":"tiling","extra":true}}"#,
            ErrorCode::InvalidParams,
            Some("request-1"),
        );
    }

    #[test]
    fn mode_values_are_case_sensitive() {
        assert_error(
            r#"{"version":1,"id":"request-1","method":"set_default_mode","params":{"mode":"Windows"}}"#,
            ErrorCode::InvalidParams,
            Some("request-1"),
        );
    }

    #[test]
    fn placement_is_not_a_mode_alias() {
        assert_error(
            r#"{"version":1,"id":"request-1","method":"set_default_mode","params":{"mode":"floating"}}"#,
            ErrorCode::InvalidParams,
            Some("request-1"),
        );
    }

    #[test]
    fn empty_workspace_id_is_invalid_params() {
        assert_error(
            r#"{"version":1,"id":"request-1","method":"get_workspace_mode","params":{"workspace_id":""}}"#,
            ErrorCode::InvalidParams,
            Some("request-1"),
        );
    }

    #[test]
    fn valid_correlation_id_is_echoed_on_parameter_error() {
        assert_error(
            r#"{"version":1,"id":"correlation","method":"set_default_mode","params":{"mode":3}}"#,
            ErrorCode::InvalidParams,
            Some("correlation"),
        );
    }

    #[test]
    fn unknown_parameter_field_is_invalid_params() {
        assert_error(
            r#"{"version":1,"id":"request-1","method":"status","params":{"unexpected":true}}"#,
            ErrorCode::InvalidParams,
            Some("request-1"),
        );
    }

    #[test]
    fn parses_all_parameterized_request_variants() {
        let cases = [
            (
                json!({"version": 1, "id": "1", "method": "set_default_mode", "params": {"mode": "windows"}}),
                RequestMethod::SetDefaultMode {
                    mode: ProtocolMode::Windows,
                },
            ),
            (
                json!({"version": 1, "id": "2", "method": "get_workspace_mode", "params": {"workspace_id": "opaque"}}),
                RequestMethod::GetWorkspaceMode {
                    workspace_id: "opaque".into(),
                },
            ),
            (
                json!({"version": 1, "id": "3", "method": "set_workspace_mode", "params": {"workspace_id": "opaque", "mode": "tiling"}}),
                RequestMethod::SetWorkspaceMode {
                    workspace_id: "opaque".into(),
                    mode: ProtocolMode::Tiling,
                },
            ),
            (
                json!({"version": 1, "id": "4", "method": "clear_workspace_mode", "params": {"workspace_id": "opaque"}}),
                RequestMethod::ClearWorkspaceMode {
                    workspace_id: "opaque".into(),
                },
            ),
            (
                json!({"version": 1, "id": "5", "method": "apply_workspace_mode", "params": {"workspace_id": "opaque"}}),
                RequestMethod::ApplyWorkspaceMode {
                    workspace_id: "opaque".into(),
                },
            ),
        ];

        for (value, expected) in cases {
            let request = parse_request(&value.to_string()).expect("request should parse");
            assert_eq!(request.method(), &expected);
        }
    }
}
