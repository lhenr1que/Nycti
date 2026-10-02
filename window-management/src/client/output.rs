//! Rendering of results and errors.
//!
//! The human format is plain text with no promised stability. Entries appear in
//! the order the daemon sent them: sorting would mean interpreting tokens,
//! which are opaque to clients. With `--json` the protocol `result` is printed
//! as one JSON line (re-serialized from the typed result, so fields this
//! client does not know are not repeated).

use std::fmt::Write as _;

use serde_json::json;

use crate::protocol::{ErrorCode, ProtocolMode, ProtocolPlacement, ProtocolResult};

const fn mode(mode: ProtocolMode) -> &'static str {
    match mode {
        ProtocolMode::Tiling => "tiling",
        ProtocolMode::Windows => "windows",
    }
}

const fn placement(placement: ProtocolPlacement) -> &'static str {
    match placement {
        ProtocolPlacement::Tiled => "tiled",
        ProtocolPlacement::Floating => "floating",
    }
}

const fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// Returns the stable wire name of a protocol error code.
pub const fn error_code_name(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::InvalidRequest => "invalid_request",
        ErrorCode::UnsupportedVersion => "unsupported_version",
        ErrorCode::UnknownMethod => "unknown_method",
        ErrorCode::InvalidParams => "invalid_params",
        ErrorCode::UnknownWorkspace => "unknown_workspace",
        ErrorCode::UnknownWindow => "unknown_window",
        ErrorCode::CompositorUnavailable => "compositor_unavailable",
        ErrorCode::ActionFailed => "action_failed",
        ErrorCode::ObservedStateUnavailable => "observed_state_unavailable",
        ErrorCode::InconsistentObservedState => "inconsistent_observed_state",
        ErrorCode::InternalError => "internal_error",
    }
}

/// Renders a result as plain text, one entry per line, each ending in LF.
pub fn render_human(result: &ProtocolResult) -> String {
    let mut text = String::new();
    // Writing to a `String` cannot fail.
    match result {
        ProtocolResult::Status(status) => {
            let _ = writeln!(
                text,
                "service={}  protocol_version={}",
                status.service, status.protocol_version
            );
        }
        ProtocolResult::Mode(result) => {
            let _ = writeln!(text, "default={}", mode(result.mode));
        }
        ProtocolResult::Workspaces(result) => {
            for workspace in &result.workspaces {
                let _ = writeln!(
                    text,
                    "{}  {}  default={}  explicit={}  effective={}",
                    workspace.workspace_id,
                    if workspace.present {
                        "present"
                    } else {
                        "absent"
                    },
                    mode(workspace.default_mode),
                    workspace.explicit_mode.map_or("none", mode),
                    mode(workspace.effective_mode),
                );
            }
        }
        ProtocolResult::WorkspaceMode(result) => {
            let _ = writeln!(
                text,
                "{}  default={}  explicit={}  effective={}",
                result.workspace_id,
                mode(result.default_mode),
                result.explicit_mode.map_or("none", mode),
                mode(result.effective_mode),
            );
        }
        ProtocolResult::AppliedWorkspaceMode(result) => {
            let _ = writeln!(
                text,
                "{}  applied  effective={}",
                result.workspace_id,
                mode(result.effective_mode),
            );
        }
        ProtocolResult::Windows(result) => {
            for window in &result.windows {
                let _ = writeln!(
                    text,
                    "{}  workspace={}  {}  fullscreen={}  focused={}",
                    window.window_id,
                    window.workspace_id,
                    placement(window.placement),
                    yes_no(window.fullscreen),
                    yes_no(window.focused),
                );
            }
        }
    }
    text
}

/// Renders the protocol `result` object as one JSON line ending in LF.
pub fn render_json(result: &ProtocolResult) -> String {
    let mut line = serde_json::to_string(result)
        .expect("closed result types contain only infallibly serializable JSON values");
    line.push('\n');
    line
}

/// Renders the `--json` error line: `{"error":{"code":"…","message":"…"}}`.
pub fn render_error_json(code: &str, message: &str) -> String {
    format!("{}\n", json!({"error": {"code": code, "message": message}}))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::protocol::{
        AppliedWorkspaceModeResult, ModeResult, StatusResult, WindowResult, WindowsResult,
        WorkspaceModeResult, WorkspaceResult, WorkspacesResult,
    };

    fn workspaces() -> ProtocolResult {
        ProtocolResult::Workspaces(WorkspacesResult {
            workspaces: vec![
                WorkspaceResult {
                    workspace_id: "w:2".into(),
                    present: true,
                    default_mode: ProtocolMode::Tiling,
                    explicit_mode: Some(ProtocolMode::Windows),
                    effective_mode: ProtocolMode::Windows,
                },
                WorkspaceResult {
                    workspace_id: "w:1".into(),
                    present: false,
                    default_mode: ProtocolMode::Tiling,
                    explicit_mode: None,
                    effective_mode: ProtocolMode::Tiling,
                },
            ],
        })
    }

    fn windows() -> ProtocolResult {
        ProtocolResult::Windows(WindowsResult {
            windows: vec![
                WindowResult {
                    window_id: "win:7".into(),
                    workspace_id: "w:2".into(),
                    placement: ProtocolPlacement::Floating,
                    fullscreen: false,
                    focused: true,
                },
                WindowResult {
                    window_id: "win:3".into(),
                    workspace_id: "w:1".into(),
                    placement: ProtocolPlacement::Tiled,
                    fullscreen: true,
                    focused: false,
                },
            ],
        })
    }

    #[test]
    fn workspaces_are_listed_in_the_received_order_with_keywords() {
        let text = render_human(&workspaces());
        let lines: Vec<&str> = text.lines().collect();

        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("w:2"), "received order is kept");
        for keyword in [
            "present",
            "default=tiling",
            "explicit=windows",
            "effective=windows",
        ] {
            assert!(lines[0].contains(keyword), "{keyword} in {}", lines[0]);
        }
        assert!(lines[1].starts_with("w:1"));
        for keyword in ["absent", "explicit=none", "effective=tiling"] {
            assert!(lines[1].contains(keyword), "{keyword} in {}", lines[1]);
        }
    }

    #[test]
    fn windows_are_listed_in_the_received_order_with_keywords() {
        let text = render_human(&windows());
        let lines: Vec<&str> = text.lines().collect();

        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("win:7"), "received order is kept");
        for keyword in ["workspace=w:2", "floating", "fullscreen=no", "focused=yes"] {
            assert!(lines[0].contains(keyword), "{keyword} in {}", lines[0]);
        }
        for keyword in ["workspace=w:1", "tiled", "fullscreen=yes", "focused=no"] {
            assert!(lines[1].contains(keyword), "{keyword} in {}", lines[1]);
        }
    }

    #[test]
    fn single_results_name_what_they_show() {
        let status = render_human(&ProtocolResult::Status(StatusResult {
            protocol_version: 1,
            service: "nycti-windowd".into(),
        }));
        assert!(status.contains("nycti-windowd") && status.contains("protocol_version=1"));

        let default = render_human(&ProtocolResult::Mode(ModeResult {
            mode: ProtocolMode::Windows,
        }));
        assert!(default.contains("default=windows"));

        let workspace = render_human(&ProtocolResult::WorkspaceMode(WorkspaceModeResult {
            workspace_id: "w:2".into(),
            default_mode: ProtocolMode::Tiling,
            explicit_mode: None,
            effective_mode: ProtocolMode::Tiling,
        }));
        assert!(workspace.starts_with("w:2"));
        assert!(workspace.contains("explicit=none"));

        let applied = render_human(&ProtocolResult::AppliedWorkspaceMode(
            AppliedWorkspaceModeResult {
                workspace_id: "w:2".into(),
                effective_mode: ProtocolMode::Windows,
            },
        ));
        assert!(applied.contains("applied") && applied.contains("effective=windows"));
    }

    #[test]
    fn empty_lists_print_nothing() {
        assert_eq!(
            render_human(&ProtocolResult::Windows(WindowsResult {
                windows: Vec::new()
            })),
            ""
        );
        assert_eq!(
            render_human(&ProtocolResult::Workspaces(WorkspacesResult {
                workspaces: Vec::new()
            })),
            ""
        );
    }

    #[test]
    fn json_is_one_line_with_the_protocol_result_object() {
        let line = render_json(&workspaces());

        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
        let value: Value = serde_json::from_str(&line).expect("output should be JSON");
        assert_eq!(value["workspaces"][0]["workspace_id"], "w:2");
        assert_eq!(value["workspaces"][1]["explicit_mode"], Value::Null);
        assert!(value.get("ok").is_none() && value.get("version").is_none());
    }

    #[test]
    fn json_keeps_the_field_order_of_the_specification() {
        let line = render_json(&workspaces());

        assert!(line.starts_with(
            r#"{"workspaces":[{"workspace_id":"w:2","present":true,"default_mode":"tiling","explicit_mode":"windows","effective_mode":"windows"}"#
        ));
    }

    #[test]
    fn the_json_error_line_has_the_documented_shape() {
        let line = render_error_json("unknown_workspace", "workspace is not known");

        assert_eq!(
            line,
            "{\"error\":{\"code\":\"unknown_workspace\",\"message\":\"workspace is not known\"}}\n"
        );
    }

    #[test]
    fn every_error_code_has_its_wire_name() {
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
            assert_eq!(
                serde_json::to_value(code).expect("code should serialize"),
                error_code_name(code)
            );
        }
    }
}
