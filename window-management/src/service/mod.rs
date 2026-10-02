//! Compositor-independent protocol service backed exclusively by `WindowManager`.

use std::collections::HashMap;

use crate::backend::{BackendError, WindowBackend, WindowId, WorkspaceId};
use crate::core::{WindowManager, WorkspaceModeResolution};
use crate::protocol::{
    AppliedWorkspaceModeResult, ErrorCode, ModeResult, PROTOCOL_VERSION, ProtocolError,
    ProtocolMode, ProtocolRequest, ProtocolResponse, ProtocolResult, RequestMethod, StatusResult,
    WindowResult, WindowsResult, WorkspaceModeResult, WorkspaceResult, WorkspacesResult,
    parse_request, parse_request_bytes, serialize_response,
};

/// Handles validated protocol operations through one owned `WindowManager`.
pub struct WindowManagementService<B: WindowBackend> {
    manager: WindowManager<B>,
    identities: ExternalIdentityRegistry,
}

impl<B: WindowBackend> WindowManagementService<B> {
    /// Creates an in-memory service around the sole owner of the backend.
    pub fn new(manager: WindowManager<B>) -> Self {
        Self {
            manager,
            identities: ExternalIdentityRegistry::new(),
        }
    }

    /// Handles one parsed request and returns a typed response.
    pub fn handle_request(&mut self, request: ProtocolRequest) -> ProtocolResponse {
        let ProtocolRequest { id, method } = request;
        let result = self.dispatch(method);

        match result {
            Ok(result) => ProtocolResponse::success(id, result),
            Err(error) => ProtocolResponse::from_protocol_error(Some(id), error),
        }
    }

    /// Parses, handles, and serializes one complete request line.
    ///
    /// The input may contain JSON's trailing whitespace, including one newline.
    /// This method does not frame streams or read from sockets.
    pub fn handle_json_line(&mut self, line: &str) -> Result<String, serde_json::Error> {
        let response = match parse_request(line) {
            Ok(request) => self.handle_request(request),
            Err(response) => response,
        };
        serialize_response(&response)
    }

    pub(crate) fn handle_json_bytes(&mut self, line: &[u8]) -> String {
        let response = match parse_request_bytes(line) {
            Ok(request) => self.handle_request(request),
            Err(response) => response,
        };

        serialize_response(&response).expect(
            "closed protocol response types contain only infallibly serializable JSON values",
        )
    }

    fn dispatch(&mut self, method: RequestMethod) -> Result<ProtocolResult, ProtocolError> {
        match method {
            RequestMethod::Status => Ok(ProtocolResult::Status(StatusResult {
                protocol_version: PROTOCOL_VERSION,
                service: "nycti-windowd",
            })),
            RequestMethod::GetDefaultMode => Ok(ProtocolResult::Mode(ModeResult {
                mode: self.manager.default_mode().into(),
            })),
            RequestMethod::SetDefaultMode { mode } => {
                self.manager.set_default_mode(mode.into());
                Ok(ProtocolResult::Mode(ModeResult {
                    mode: self.manager.default_mode().into(),
                }))
            }
            RequestMethod::ListWorkspaces => self.list_workspaces(),
            RequestMethod::GetWorkspaceMode { workspace_id } => {
                let internal_id = self.resolve_workspace(&workspace_id)?;
                Ok(ProtocolResult::WorkspaceMode(workspace_mode_result(
                    workspace_id,
                    self.manager.workspace_mode(internal_id),
                )))
            }
            RequestMethod::SetWorkspaceMode { workspace_id, mode } => {
                let internal_id = self.resolve_workspace(&workspace_id)?;
                self.manager.set_workspace_mode(internal_id, mode.into());
                Ok(ProtocolResult::WorkspaceMode(workspace_mode_result(
                    workspace_id,
                    self.manager.workspace_mode(internal_id),
                )))
            }
            RequestMethod::ClearWorkspaceMode { workspace_id } => {
                let internal_id = self.resolve_workspace(&workspace_id)?;
                self.manager.clear_workspace_mode(internal_id);
                Ok(ProtocolResult::WorkspaceMode(workspace_mode_result(
                    workspace_id,
                    self.manager.workspace_mode(internal_id),
                )))
            }
            RequestMethod::ApplyWorkspaceMode { workspace_id } => {
                let internal_id = self.resolve_workspace(&workspace_id)?;
                let effective_mode = self.manager.workspace_mode(internal_id).effective_mode();
                self.manager
                    .apply_workspace_mode(internal_id)
                    .map_err(protocol_backend_error)?;
                Ok(ProtocolResult::AppliedWorkspaceMode(
                    AppliedWorkspaceModeResult {
                        workspace_id,
                        effective_mode: effective_mode.into(),
                    },
                ))
            }
            RequestMethod::ListWindows => self.list_windows(),
        }
    }

    fn list_workspaces(&mut self) -> Result<ProtocolResult, ProtocolError> {
        let observations = self
            .manager
            .list_workspaces()
            .map_err(protocol_backend_error)?;
        let mut workspaces = Vec::with_capacity(observations.len());

        for observation in observations {
            let workspace_id = self.identities.workspace_token(observation.id())?;
            let resolution = self.manager.workspace_mode(observation.id());
            workspaces.push(WorkspaceResult {
                workspace_id,
                present: observation.is_present(),
                default_mode: resolution.default_mode().into(),
                explicit_mode: resolution.explicit_mode().map(ProtocolMode::from),
                effective_mode: resolution.effective_mode().into(),
            });
        }

        Ok(ProtocolResult::Workspaces(WorkspacesResult { workspaces }))
    }

    fn list_windows(&mut self) -> Result<ProtocolResult, ProtocolError> {
        let observations = self
            .manager
            .list_windows()
            .map_err(protocol_backend_error)?;
        let mut windows = Vec::with_capacity(observations.len());

        for observation in observations {
            let workspace_id = self
                .identities
                .workspace_token(observation.workspace_id())?;
            let window_id = self.identities.window_token(observation.id())?;
            windows.push(WindowResult {
                window_id,
                workspace_id,
                placement: observation.placement().into(),
                fullscreen: observation.is_fullscreen(),
                focused: observation.is_focused(),
            });
        }

        Ok(ProtocolResult::Windows(WindowsResult { windows }))
    }

    fn resolve_workspace(&self, token: &str) -> Result<WorkspaceId, ProtocolError> {
        self.identities.resolve_workspace(token).ok_or_else(|| {
            ProtocolError::new(ErrorCode::UnknownWorkspace, "workspace is not known")
        })
    }
}

fn workspace_mode_result(
    workspace_id: String,
    resolution: WorkspaceModeResolution,
) -> WorkspaceModeResult {
    WorkspaceModeResult {
        workspace_id,
        default_mode: resolution.default_mode().into(),
        explicit_mode: resolution.explicit_mode().map(ProtocolMode::from),
        effective_mode: resolution.effective_mode().into(),
    }
}

fn protocol_backend_error(error: BackendError) -> ProtocolError {
    let (code, message) = match error {
        BackendError::UnknownWorkspace(_) => {
            (ErrorCode::UnknownWorkspace, "workspace is not known")
        }
        BackendError::UnknownWindow(_) => (ErrorCode::UnknownWindow, "window is not known"),
        BackendError::CompositorUnavailable => (
            ErrorCode::CompositorUnavailable,
            "compositor is unavailable",
        ),
        BackendError::ActionFailed => (ErrorCode::ActionFailed, "window placement action failed"),
        BackendError::ObservedStateUnavailable => (
            ErrorCode::ObservedStateUnavailable,
            "observed window state is unavailable",
        ),
        BackendError::InconsistentObservedState => (
            ErrorCode::InconsistentObservedState,
            "observed window state is inconsistent",
        ),
    };
    ProtocolError::new(code, message)
}

struct ExternalIdentityRegistry {
    next_workspace_token: u64,
    next_window_token: u64,
    workspace_tokens: HashMap<WorkspaceId, String>,
    workspaces_by_token: HashMap<String, WorkspaceId>,
    window_tokens: HashMap<WindowId, String>,
    windows_by_token: HashMap<String, WindowId>,
}

impl ExternalIdentityRegistry {
    fn new() -> Self {
        Self {
            next_workspace_token: 1,
            next_window_token: 1,
            workspace_tokens: HashMap::new(),
            workspaces_by_token: HashMap::new(),
            window_tokens: HashMap::new(),
            windows_by_token: HashMap::new(),
        }
    }

    fn workspace_token(&mut self, id: WorkspaceId) -> Result<String, ProtocolError> {
        if let Some(token) = self.workspace_tokens.get(&id) {
            return Ok(token.clone());
        }

        let token = format!("w:{}", self.next_workspace_token);
        self.next_workspace_token = self.next_workspace_token.checked_add(1).ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::InternalError,
                "workspace identity capacity is exhausted",
            )
        })?;
        self.workspace_tokens.insert(id, token.clone());
        self.workspaces_by_token.insert(token.clone(), id);
        Ok(token)
    }

    fn window_token(&mut self, id: WindowId) -> Result<String, ProtocolError> {
        if let Some(token) = self.window_tokens.get(&id) {
            return Ok(token.clone());
        }

        let token = format!("win:{}", self.next_window_token);
        self.next_window_token = self.next_window_token.checked_add(1).ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::InternalError,
                "window identity capacity is exhausted",
            )
        })?;
        self.window_tokens.insert(id, token.clone());
        self.windows_by_token.insert(token.clone(), id);
        Ok(token)
    }

    fn resolve_workspace(&self, token: &str) -> Option<WorkspaceId> {
        self.workspaces_by_token.get(token).copied()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use serde_json::{Value, json};

    use super::*;
    use crate::backend::fake::FakeBackend;
    use crate::core::{WindowPlacement, WorkspaceMode};

    fn service_with_backend(
        backend: FakeBackend,
        default_mode: WorkspaceMode,
    ) -> WindowManagementService<FakeBackend> {
        WindowManagementService::new(WindowManager::new(backend, default_mode))
    }

    fn request(service: &mut WindowManagementService<FakeBackend>, value: Value) -> Value {
        let line = service
            .handle_json_line(&value.to_string())
            .expect("closed response should serialize");
        assert!(line.ends_with('\n'));
        serde_json::from_str(&line).expect("serialized response should be JSON")
    }

    fn call(
        service: &mut WindowManagementService<FakeBackend>,
        id: &str,
        method: &str,
        params: Value,
    ) -> Value {
        request(
            service,
            json!({"version": 1, "id": id, "method": method, "params": params}),
        )
    }

    fn workspace_token(service: &mut WindowManagementService<FakeBackend>) -> String {
        call(service, "list", "list_workspaces", json!({}))["result"]["workspaces"][0]
            ["workspace_id"]
            .as_str()
            .expect("workspace token should be a string")
            .to_owned()
    }

    #[test]
    fn status_reports_service_without_backend_metadata() {
        let mut service = service_with_backend(FakeBackend::new(), WorkspaceMode::Tiling);

        let response = call(&mut service, "status-id", "status", json!({}));

        assert_eq!(
            response,
            json!({
                "version": 1,
                "id": "status-id",
                "ok": true,
                "result": {"protocol_version": 1, "service": "nycti-windowd"}
            })
        );
    }

    #[test]
    fn default_mode_can_be_read_and_changed_without_applying_placement() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        backend
            .add_window(workspace, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept window");
        let mut service = service_with_backend(backend, WorkspaceMode::Tiling);

        let before = call(&mut service, "before", "list_windows", json!({}));
        let set = call(
            &mut service,
            "set",
            "set_default_mode",
            json!({"mode": "windows"}),
        );
        let get = call(&mut service, "get", "get_default_mode", json!({}));
        let after = call(&mut service, "after", "list_windows", json!({}));

        assert_eq!(set["result"]["mode"], "windows");
        assert_eq!(get["result"]["mode"], "windows");
        assert_eq!(
            before["result"]["windows"][0]["placement"],
            after["result"]["windows"][0]["placement"]
        );
        assert_eq!(after["result"]["windows"][0]["placement"], "tiled");
    }

    #[test]
    fn workspace_set_and_clear_do_not_apply_placement() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        backend
            .add_window(workspace, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept window");
        let mut service = service_with_backend(backend, WorkspaceMode::Tiling);
        let token = workspace_token(&mut service);
        let before = call(&mut service, "before", "list_windows", json!({}));

        let set = call(
            &mut service,
            "set",
            "set_workspace_mode",
            json!({"workspace_id": token, "mode": "windows"}),
        );
        let middle = call(&mut service, "middle", "list_windows", json!({}));
        let clear = call(
            &mut service,
            "clear",
            "clear_workspace_mode",
            json!({"workspace_id": token}),
        );
        let after = call(&mut service, "after", "list_windows", json!({}));

        assert_eq!(set["result"]["effective_mode"], "windows");
        assert_eq!(clear["result"]["explicit_mode"], Value::Null);
        assert_eq!(before["result"]["windows"], middle["result"]["windows"]);
        assert_eq!(middle["result"]["windows"], after["result"]["windows"]);
        assert_eq!(after["result"]["windows"][0]["placement"], "tiled");
    }

    #[test]
    fn clear_without_existing_override_succeeds() {
        let mut backend = FakeBackend::new();
        backend.add_workspace(true);
        let mut service = service_with_backend(backend, WorkspaceMode::Tiling);
        let token = workspace_token(&mut service);

        let response = call(
            &mut service,
            "clear",
            "clear_workspace_mode",
            json!({"workspace_id": token}),
        );

        assert_eq!(response["ok"], true);
        assert_eq!(response["result"]["explicit_mode"], Value::Null);
        assert_eq!(response["result"]["effective_mode"], "tiling");
    }

    #[test]
    fn apply_flows_through_manager_planner_and_fake_backend() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        backend
            .add_window(workspace, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept window");
        let mut service = service_with_backend(backend, WorkspaceMode::Tiling);
        let token = workspace_token(&mut service);

        call(
            &mut service,
            "set",
            "set_workspace_mode",
            json!({"workspace_id": token, "mode": "windows"}),
        );
        let applied = call(
            &mut service,
            "apply",
            "apply_workspace_mode",
            json!({"workspace_id": token}),
        );
        let windows = call(&mut service, "windows", "list_windows", json!({}));

        assert_eq!(applied["result"]["workspace_id"], token);
        assert_eq!(applied["result"]["effective_mode"], "windows");
        assert_eq!(windows["result"]["windows"][0]["placement"], "floating");
    }

    #[test]
    fn action_failure_maps_without_backend_diagnostics() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        let window = backend
            .add_window(workspace, WindowPlacement::Floating, false, false)
            .expect("known workspace should accept window");
        backend.fail_actions_for(window, BackendError::ActionFailed);
        let mut service = service_with_backend(backend, WorkspaceMode::Tiling);
        let token = workspace_token(&mut service);

        let response = call(
            &mut service,
            "apply",
            "apply_workspace_mode",
            json!({"workspace_id": token}),
        );

        assert_eq!(response["error"]["code"], "action_failed");
        assert_eq!(
            response["error"]["message"],
            "window placement action failed"
        );
        assert!(
            !response["error"]["message"]
                .as_str()
                .expect("message should be text")
                .contains("ActionFailed")
        );
    }

    #[test]
    fn unknown_workspace_token_is_not_parsed_or_synthesized() {
        let mut backend = FakeBackend::new();
        backend.add_workspace(true);
        let mut service = service_with_backend(backend, WorkspaceMode::Tiling);
        workspace_token(&mut service);

        let response = call(
            &mut service,
            "unknown",
            "get_workspace_mode",
            json!({"workspace_id": "w:999999"}),
        );

        assert_eq!(response["error"]["code"], "unknown_workspace");
    }

    #[test]
    fn workspace_listing_reports_default_explicit_and_effective_modes() {
        let mut backend = FakeBackend::new();
        backend.add_workspace(true);
        backend.add_workspace(true);
        let mut service = service_with_backend(backend, WorkspaceMode::Tiling);
        let first = call(&mut service, "first", "list_workspaces", json!({}));
        let workspaces = first["result"]["workspaces"]
            .as_array()
            .expect("workspaces should be an array");
        let tokens = workspaces
            .iter()
            .map(|workspace| {
                workspace["workspace_id"]
                    .as_str()
                    .expect("token should be text")
                    .to_owned()
            })
            .collect::<HashSet<_>>();
        assert_eq!(tokens.len(), 2);
        let configured_token = tokens
            .iter()
            .next()
            .expect("one workspace token should be available")
            .clone();
        let inherited_token = tokens
            .iter()
            .find(|token| **token != configured_token)
            .expect("the other workspace token should be available")
            .clone();

        call(
            &mut service,
            "set",
            "set_workspace_mode",
            json!({"workspace_id": configured_token, "mode": "windows"}),
        );
        let listed = call(&mut service, "second", "list_workspaces", json!({}));
        let workspaces = listed["result"]["workspaces"]
            .as_array()
            .expect("workspaces should be an array");
        let inherited = workspaces
            .iter()
            .find(|workspace| workspace["workspace_id"].as_str() == Some(inherited_token.as_str()))
            .expect("inherited workspace should remain listed");
        let configured = workspaces
            .iter()
            .find(|workspace| workspace["workspace_id"].as_str() == Some(configured_token.as_str()))
            .expect("configured workspace should remain listed");

        assert!(inherited_token.starts_with("w:"));
        assert!(configured_token.starts_with("w:"));
        assert_eq!(inherited["default_mode"], "tiling");
        assert_eq!(inherited["explicit_mode"], Value::Null);
        assert_eq!(inherited["effective_mode"], "tiling");
        assert_eq!(configured["default_mode"], "tiling");
        assert_eq!(configured["explicit_mode"], "windows");
        assert_eq!(configured["effective_mode"], "windows");
    }

    #[test]
    fn workspace_and_window_tokens_are_stable_distinct_and_shared_across_lists() {
        let mut backend = FakeBackend::new();
        let first_workspace = backend.add_workspace(true);
        let second_workspace = backend.add_workspace(true);
        backend
            .add_window(first_workspace, WindowPlacement::Tiled, false, true)
            .expect("known workspace should accept window");
        backend
            .add_window(second_workspace, WindowPlacement::Floating, true, false)
            .expect("known workspace should accept window");
        let mut service = service_with_backend(backend, WorkspaceMode::Tiling);

        let workspaces_first = call(&mut service, "w1", "list_workspaces", json!({}));
        let windows_first = call(&mut service, "win1", "list_windows", json!({}));
        let workspaces_second = call(&mut service, "w2", "list_workspaces", json!({}));
        let windows_second = call(&mut service, "win2", "list_windows", json!({}));

        let workspace_tokens_first = workspaces_first["result"]["workspaces"]
            .as_array()
            .expect("workspaces should be an array")
            .iter()
            .map(|workspace| {
                workspace["workspace_id"]
                    .as_str()
                    .expect("workspace token should be text")
                    .to_owned()
            })
            .collect::<HashSet<_>>();
        let workspace_tokens_second = workspaces_second["result"]["workspaces"]
            .as_array()
            .expect("workspaces should be an array")
            .iter()
            .map(|workspace| {
                workspace["workspace_id"]
                    .as_str()
                    .expect("workspace token should be text")
                    .to_owned()
            })
            .collect::<HashSet<_>>();
        let windows_by_token_first = windows_first["result"]["windows"]
            .as_array()
            .expect("windows should be an array")
            .iter()
            .map(|window| {
                let window_id = window["window_id"]
                    .as_str()
                    .expect("window token should be text")
                    .to_owned();
                let workspace_id = window["workspace_id"]
                    .as_str()
                    .expect("workspace token should be text")
                    .to_owned();
                (window_id, workspace_id)
            })
            .collect::<HashMap<_, _>>();
        let windows_by_token_second = windows_second["result"]["windows"]
            .as_array()
            .expect("windows should be an array")
            .iter()
            .map(|window| {
                let window_id = window["window_id"]
                    .as_str()
                    .expect("window token should be text")
                    .to_owned();
                let workspace_id = window["workspace_id"]
                    .as_str()
                    .expect("workspace token should be text")
                    .to_owned();
                (window_id, workspace_id)
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(workspace_tokens_first.len(), 2);
        assert_eq!(workspace_tokens_first, workspace_tokens_second);
        assert!(
            workspace_tokens_first
                .iter()
                .all(|token| token.starts_with("w:"))
        );
        assert_eq!(windows_by_token_first.len(), 2);
        assert_eq!(windows_by_token_first, windows_by_token_second);
        assert!(
            windows_by_token_first
                .keys()
                .all(|token| token.starts_with("win:"))
        );
        assert!(
            windows_by_token_first
                .values()
                .all(|token| workspace_tokens_first.contains(token))
        );
    }

    #[test]
    fn listing_windows_registers_its_workspace_before_workspace_listing() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        backend
            .add_window(workspace, WindowPlacement::Tiled, false, false)
            .expect("known workspace should accept window");
        let mut service = service_with_backend(backend, WorkspaceMode::Tiling);

        let windows = call(&mut service, "windows", "list_windows", json!({}));
        let workspaces = call(&mut service, "workspaces", "list_workspaces", json!({}));

        assert_eq!(
            windows["result"]["windows"][0]["workspace_id"],
            workspaces["result"]["workspaces"][0]["workspace_id"]
        );
    }

    #[test]
    fn list_windows_emits_only_normalized_fields_and_canonical_placement() {
        let mut backend = FakeBackend::new();
        let workspace = backend.add_workspace(true);
        backend
            .add_window(workspace, WindowPlacement::Floating, true, true)
            .expect("known workspace should accept window");
        let mut service = service_with_backend(backend, WorkspaceMode::Tiling);

        let response = call(&mut service, "windows", "list_windows", json!({}));
        let window = response["result"]["windows"][0]
            .as_object()
            .expect("window should be an object");

        assert_eq!(window.len(), 5);
        assert_eq!(window["placement"], "floating");
        assert_eq!(window["fullscreen"], true);
        assert_eq!(window["focused"], true);
        assert!(window.contains_key("window_id"));
        assert!(window.contains_key("workspace_id"));
    }

    #[test]
    fn parser_errors_are_serialized_without_invoking_a_handler() {
        let mut service = service_with_backend(FakeBackend::new(), WorkspaceMode::Tiling);

        let line = service
            .handle_json_line("not json")
            .expect("error response should serialize");
        let response: Value = serde_json::from_str(&line).expect("response should be JSON");

        assert_eq!(response["version"], 1);
        assert_eq!(response["id"], Value::Null);
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], "invalid_request");
        assert!(response.get("result").is_none());
    }
}
