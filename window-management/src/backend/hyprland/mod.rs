//! Native Hyprland backend, response parsing, and snapshot normalization.

mod snapshot;
mod transport;
mod wire;

#[cfg(test)]
mod live_tests;

use super::{BackendError, WindowBackend, WindowId, WindowObservation, WorkspaceObservation};
use crate::core::WindowPlacement;
use snapshot::{HyprlandSnapshot, HyprlandSnapshotSource, NativeStableId};
use transport::HyprlandTransport;

const ACTION_SUCCESS_RESPONSE: &[u8] = b"ok";

/// Native synchronous backend for Hyprland 0.56.2.
pub struct HyprlandBackend {
    transport: HyprlandTransport,
    snapshots: HyprlandSnapshotSource,
}

#[derive(Clone, Copy)]
enum HyprlandAction {
    EnsureTiled,
    EnsureFloating,
    Focus,
}

impl HyprlandAction {
    fn request(self, stable_id: NativeStableId) -> Vec<u8> {
        match self {
            Self::EnsureTiled => format!(
                "/dispatch hl.dsp.window.float({{ window = \"stableid:{stable_id:x}\", action = \"disable\" }})"
            )
            .into_bytes(),
            Self::EnsureFloating => format!(
                "/dispatch hl.dsp.window.float({{ window = \"stableid:{stable_id:x}\", action = \"enable\" }})"
            )
            .into_bytes(),
            Self::Focus => format!(
                "/dispatch hl.dsp.focus({{ window = \"stableid:{stable_id:x}\" }})"
            )
            .into_bytes(),
        }
    }

    fn postcondition_is_satisfied(self, window: &WindowObservation) -> bool {
        match self {
            Self::EnsureTiled => window.placement() == WindowPlacement::Tiled,
            Self::EnsureFloating => window.placement() == WindowPlacement::Floating,
            Self::Focus => window.is_focused(),
        }
    }
}

impl HyprlandBackend {
    /// Creates a backend using the current Hyprland session environment.
    pub fn from_env() -> Result<Self, BackendError> {
        Ok(Self::new(HyprlandTransport::from_env()?))
    }

    fn new(transport: HyprlandTransport) -> Self {
        Self {
            transport,
            snapshots: HyprlandSnapshotSource::new(),
        }
    }

    fn snapshot(&mut self) -> Result<HyprlandSnapshot, BackendError> {
        let workspaces = self.transport.request_workspaces()?;
        let clients = self.transport.request_clients()?;
        let active_window = self.transport.request_active_window()?;

        self.snapshots
            .normalize(&workspaces, &clients, &active_window)
    }

    fn perform_action(
        &mut self,
        window_id: WindowId,
        action: HyprlandAction,
    ) -> Result<(), BackendError> {
        let stable_id = self
            .snapshots
            .native_stable_id(window_id)
            .ok_or(BackendError::UnknownWindow(window_id))?;
        let request = action.request(stable_id);
        let response = self.transport.request_bytes(&request)?;

        if response.as_slice() != ACTION_SUCCESS_RESPONSE {
            return Err(BackendError::ActionFailed);
        }

        let snapshot = self.snapshot()?;
        let window = snapshot
            .windows
            .iter()
            .find(|window| window.id() == window_id)
            .ok_or(BackendError::UnknownWindow(window_id))?;

        if action.postcondition_is_satisfied(window) {
            Ok(())
        } else {
            Err(BackendError::ActionFailed)
        }
    }
}

impl WindowBackend for HyprlandBackend {
    fn list_workspaces(&mut self) -> Result<Vec<WorkspaceObservation>, BackendError> {
        Ok(self.snapshot()?.workspaces)
    }

    fn list_windows(&mut self) -> Result<Vec<WindowObservation>, BackendError> {
        Ok(self.snapshot()?.windows)
    }

    fn ensure_tiled(&mut self, window_id: WindowId) -> Result<(), BackendError> {
        self.perform_action(window_id, HyprlandAction::EnsureTiled)
    }

    fn ensure_floating(&mut self, window_id: WindowId) -> Result<(), BackendError> {
        self.perform_action(window_id, HyprlandAction::EnsureFloating)
    }

    fn focus_window(&mut self, window_id: WindowId) -> Result<(), BackendError> {
        self.perform_action(window_id, HyprlandAction::Focus)
    }
}

#[cfg(test)]
mod tests {
    use super::transport::test_support::{MockExchange, MockServer};
    use super::{HyprlandAction, HyprlandBackend, HyprlandTransport};
    use crate::backend::{BackendError, WindowBackend, WindowId};
    use crate::core::WindowPlacement;

    const WORKSPACES_FIXTURE: &str =
        include_str!("../../../tests/fixtures/hyprland/workspaces.json");
    const CLIENTS_FIXTURE: &str = include_str!("../../../tests/fixtures/hyprland/clients.json");
    const ACTIVE_WINDOW_FIXTURE: &str =
        include_str!("../../../tests/fixtures/hyprland/activewindow.json");

    const TARGET_STABLE_ID: &str = "01A2B3C";
    const TARGET_ADDRESS: &str = "0x100";
    const OTHER_STABLE_ID: &str = "2b";
    const OTHER_ADDRESS: &str = "0x200";

    const ENSURE_FLOATING_REQUEST: &[u8] =
        b"/dispatch hl.dsp.window.float({ window = \"stableid:1a2b3c\", action = \"enable\" })";
    const ENSURE_TILED_REQUEST: &[u8] =
        b"/dispatch hl.dsp.window.float({ window = \"stableid:1a2b3c\", action = \"disable\" })";
    const FOCUS_REQUEST: &[u8] = b"/dispatch hl.dsp.focus({ window = \"stableid:1a2b3c\" })";

    fn fixture_responses(repetitions: usize) -> Vec<MockExchange> {
        (0..repetitions)
            .flat_map(|_| {
                [
                    MockExchange::new(b"j/workspaces", WORKSPACES_FIXTURE.as_bytes().to_vec()),
                    MockExchange::new(b"j/clients", CLIENTS_FIXTURE.as_bytes().to_vec()),
                    MockExchange::new(b"j/activewindow", ACTIVE_WINDOW_FIXTURE.as_bytes().to_vec()),
                ]
            })
            .collect()
    }

    fn single_window_responses(
        workspace_id: i64,
        stable_id: &str,
        address: &str,
    ) -> Vec<MockExchange> {
        vec![
            MockExchange::new(
                b"j/workspaces",
                format!(r#"[{{"id":{workspace_id}}}]"#).into_bytes(),
            ),
            MockExchange::new(
                b"j/clients",
                format!(
                    r#"[{{"stableId":"{stable_id}","address":"{address}","workspace":{{"id":{workspace_id}}},"floating":false,"fullscreen":0}}]"#
                )
                .into_bytes(),
            ),
            MockExchange::new(
                b"j/activewindow",
                format!(r#"{{"stableId":"{stable_id}","address":"{address}"}}"#).into_bytes(),
            ),
        ]
    }

    fn action_snapshot_responses(
        target_floating: Option<bool>,
        target_focused: bool,
    ) -> Vec<MockExchange> {
        let mut clients = Vec::new();
        if let Some(floating) = target_floating {
            clients.push(format!(
                r#"{{"stableId":"{TARGET_STABLE_ID}","address":"{TARGET_ADDRESS}","workspace":{{"id":1}},"floating":{floating},"fullscreen":0}}"#
            ));
        }

        if !target_focused {
            clients.push(format!(
                r#"{{"stableId":"{OTHER_STABLE_ID}","address":"{OTHER_ADDRESS}","workspace":{{"id":1}},"floating":false,"fullscreen":0}}"#
            ));
        }

        let (active_stable_id, active_address) = if target_focused {
            (TARGET_STABLE_ID, TARGET_ADDRESS)
        } else {
            (OTHER_STABLE_ID, OTHER_ADDRESS)
        };

        vec![
            MockExchange::new(b"j/workspaces", br#"[{"id":1}]"#.to_vec()),
            MockExchange::new(
                b"j/clients",
                format!("[{}]", clients.join(",")).into_bytes(),
            ),
            MockExchange::new(
                b"j/activewindow",
                format!(r#"{{"stableId":"{active_stable_id}","address":"{active_address}"}}"#)
                    .into_bytes(),
            ),
        ]
    }

    fn backend_with_target(
        mut following_exchanges: Vec<MockExchange>,
    ) -> (MockServer, HyprlandBackend, WindowId) {
        let mut exchanges = action_snapshot_responses(Some(false), true);
        exchanges.append(&mut following_exchanges);
        let server = MockServer::start(exchanges);
        let transport = HyprlandTransport::with_socket_path(server.path().to_owned());
        let mut backend = HyprlandBackend::new(transport);
        let target = backend
            .list_windows()
            .expect("initial target snapshot should normalize")[0]
            .id();

        (server, backend, target)
    }

    fn action_and_snapshot(
        request: &'static [u8],
        target_floating: Option<bool>,
        target_focused: bool,
    ) -> Vec<MockExchange> {
        let mut exchanges = vec![MockExchange::new(request, b"ok".to_vec())];
        exchanges.extend(action_snapshot_responses(target_floating, target_focused));
        exchanges
    }

    fn assert_action_request_sequence(requests: &[Vec<u8>], action_request: &[u8]) {
        assert_eq!(requests.len(), 7);
        assert_eq!(requests[0], b"j/workspaces");
        assert_eq!(requests[1], b"j/clients");
        assert_eq!(requests[2], b"j/activewindow");
        assert_eq!(requests[3], action_request);
        assert_eq!(requests[4], b"j/workspaces");
        assert_eq!(requests[5], b"j/clients");
        assert_eq!(requests[6], b"j/activewindow");
    }

    #[test]
    fn environment_constructor_requires_no_environment_mutation() {
        match HyprlandBackend::from_env() {
            Ok(_) | Err(BackendError::CompositorUnavailable) => {}
            Err(error) => panic!("unexpected environment resolution error: {error:?}"),
        }
    }

    #[test]
    fn backend_requests_and_normalizes_a_snapshot() {
        let server = MockServer::start(fixture_responses(1));
        let transport = HyprlandTransport::with_socket_path(server.path().to_owned());
        let mut backend = HyprlandBackend::new(transport);

        let snapshot = backend
            .snapshot()
            .expect("fixture responses should produce a snapshot");
        let requests = server.finish();

        assert_eq!(
            requests,
            vec![
                b"j/workspaces".to_vec(),
                b"j/clients".to_vec(),
                b"j/activewindow".to_vec(),
            ]
        );
        assert_eq!(snapshot.workspaces.len(), 2);
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.windows[0].placement(), WindowPlacement::Tiled);
        assert_eq!(snapshot.windows[1].placement(), WindowPlacement::Floating);
        assert!(!snapshot.windows[0].is_focused());
        assert!(snapshot.windows[1].is_focused());
    }

    #[test]
    fn backend_preserves_ids_across_successive_snapshots() {
        let server = MockServer::start(fixture_responses(2));
        let transport = HyprlandTransport::with_socket_path(server.path().to_owned());
        let mut backend = HyprlandBackend::new(transport);

        let first = backend.snapshot().expect("first snapshot should normalize");
        let second = backend
            .snapshot()
            .expect("second snapshot should normalize");
        let requests = server.finish();

        assert_eq!(requests.len(), 6);
        assert_eq!(first.workspaces[0].id(), second.workspaces[0].id());
        assert_eq!(first.workspaces[1].id(), second.workspaces[1].id());
        assert_eq!(first.windows[0].id(), second.windows[0].id());
        assert_eq!(first.windows[1].id(), second.windows[1].id());
    }

    #[test]
    fn window_backend_lists_workspaces_and_windows_from_fresh_snapshots() {
        let server = MockServer::start(fixture_responses(2));
        let transport = HyprlandTransport::with_socket_path(server.path().to_owned());
        let mut backend = HyprlandBackend::new(transport);

        let workspaces = backend
            .list_workspaces()
            .expect("workspace listing should normalize");
        let windows = backend
            .list_windows()
            .expect("window listing should normalize");
        let requests = server.finish();

        assert_eq!(requests.len(), 6);
        assert_eq!(workspaces.len(), 2);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].placement(), WindowPlacement::Tiled);
        assert_eq!(windows[1].placement(), WindowPlacement::Floating);
    }

    #[test]
    fn action_payload_builder_is_byte_exact_and_canonical() {
        let server = MockServer::start(action_snapshot_responses(Some(false), true));
        let transport = HyprlandTransport::with_socket_path(server.path().to_owned());
        let mut backend = HyprlandBackend::new(transport);
        let target = backend
            .list_windows()
            .expect("initial target snapshot should normalize")[0]
            .id();
        let stable_id = backend
            .snapshots
            .native_stable_id(target)
            .expect("normalized target should retain its native stable ID");

        assert_eq!(
            HyprlandAction::EnsureFloating.request(stable_id),
            ENSURE_FLOATING_REQUEST
        );
        assert_eq!(
            HyprlandAction::EnsureTiled.request(stable_id),
            ENSURE_TILED_REQUEST
        );
        assert_eq!(HyprlandAction::Focus.request(stable_id), FOCUS_REQUEST);
        assert_eq!(server.finish().len(), 3);
    }

    #[test]
    fn active_window_parse_failure_does_not_partially_update_identities() {
        let mut subject_responses = fixture_responses(1);
        let mut invalid_responses = single_window_responses(99, "99", "0xfailed");
        invalid_responses[2] = MockExchange::new(b"j/activewindow", b"{".to_vec());
        subject_responses.extend(invalid_responses);
        subject_responses.extend(single_window_responses(100, "100", "0xsubject"));

        let subject_server = MockServer::start(subject_responses);
        let subject_transport =
            HyprlandTransport::with_socket_path(subject_server.path().to_owned());
        let mut subject = HyprlandBackend::new(subject_transport);

        subject
            .snapshot()
            .expect("initial subject snapshot should normalize");
        assert!(matches!(
            subject.snapshot(),
            Err(BackendError::ObservedStateUnavailable)
        ));
        let subject_after_failure = subject
            .snapshot()
            .expect("subject should recover after invalid active-window JSON");
        let subject_requests = subject_server.finish();

        let mut control_responses = fixture_responses(1);
        control_responses.extend(single_window_responses(100, "100", "0xcontrol"));
        let control_server = MockServer::start(control_responses);
        let control_transport =
            HyprlandTransport::with_socket_path(control_server.path().to_owned());
        let mut control = HyprlandBackend::new(control_transport);

        control
            .snapshot()
            .expect("initial control snapshot should normalize");
        let control_without_failure = control
            .snapshot()
            .expect("control snapshot should normalize");
        let control_requests = control_server.finish();

        assert_eq!(subject_requests.len(), 9);
        assert_eq!(control_requests.len(), 6);
        assert_eq!(
            subject_after_failure.workspaces[0].id(),
            control_without_failure.workspaces[0].id()
        );
        assert_eq!(
            subject_after_failure.windows[0].id(),
            control_without_failure.windows[0].id()
        );
    }

    #[test]
    fn ensure_floating_emits_exact_payload_and_accepts_floating_postcondition() {
        let (server, mut backend, target) = backend_with_target(action_and_snapshot(
            ENSURE_FLOATING_REQUEST,
            Some(true),
            true,
        ));

        assert_eq!(backend.ensure_floating(target), Ok(()));
        let requests = server.finish();

        assert_action_request_sequence(&requests, ENSURE_FLOATING_REQUEST);
    }

    #[test]
    fn ensure_floating_rejects_tiled_postcondition() {
        let (server, mut backend, target) = backend_with_target(action_and_snapshot(
            ENSURE_FLOATING_REQUEST,
            Some(false),
            true,
        ));

        assert_eq!(
            backend.ensure_floating(target),
            Err(BackendError::ActionFailed)
        );
        let requests = server.finish();

        assert_action_request_sequence(&requests, ENSURE_FLOATING_REQUEST);
    }

    #[test]
    fn ensure_floating_reports_missing_postcondition_target() {
        let (server, mut backend, target) =
            backend_with_target(action_and_snapshot(ENSURE_FLOATING_REQUEST, None, false));

        assert_eq!(
            backend.ensure_floating(target),
            Err(BackendError::UnknownWindow(target))
        );
        let requests = server.finish();

        assert_action_request_sequence(&requests, ENSURE_FLOATING_REQUEST);
    }

    #[test]
    fn ensure_tiled_emits_exact_payload_and_accepts_tiled_postcondition() {
        let (server, mut backend, target) =
            backend_with_target(action_and_snapshot(ENSURE_TILED_REQUEST, Some(false), true));

        assert_eq!(backend.ensure_tiled(target), Ok(()));
        let requests = server.finish();

        assert_action_request_sequence(&requests, ENSURE_TILED_REQUEST);
    }

    #[test]
    fn ensure_tiled_rejects_floating_postcondition() {
        let (server, mut backend, target) =
            backend_with_target(action_and_snapshot(ENSURE_TILED_REQUEST, Some(true), true));

        assert_eq!(
            backend.ensure_tiled(target),
            Err(BackendError::ActionFailed)
        );
        let requests = server.finish();

        assert_action_request_sequence(&requests, ENSURE_TILED_REQUEST);
    }

    #[test]
    fn ensure_tiled_reports_missing_postcondition_target() {
        let (server, mut backend, target) =
            backend_with_target(action_and_snapshot(ENSURE_TILED_REQUEST, None, false));

        assert_eq!(
            backend.ensure_tiled(target),
            Err(BackendError::UnknownWindow(target))
        );
        let requests = server.finish();

        assert_action_request_sequence(&requests, ENSURE_TILED_REQUEST);
    }

    #[test]
    fn focus_emits_exact_payload_and_accepts_focused_postcondition() {
        let (server, mut backend, target) =
            backend_with_target(action_and_snapshot(FOCUS_REQUEST, Some(false), true));

        assert_eq!(backend.focus_window(target), Ok(()));
        let requests = server.finish();

        assert_action_request_sequence(&requests, FOCUS_REQUEST);
    }

    #[test]
    fn focus_rejects_unfocused_postcondition() {
        let (server, mut backend, target) =
            backend_with_target(action_and_snapshot(FOCUS_REQUEST, Some(false), false));

        assert_eq!(
            backend.focus_window(target),
            Err(BackendError::ActionFailed)
        );
        let requests = server.finish();

        assert_action_request_sequence(&requests, FOCUS_REQUEST);
    }

    #[test]
    fn focus_reports_missing_postcondition_target() {
        let (server, mut backend, target) =
            backend_with_target(action_and_snapshot(FOCUS_REQUEST, None, false));

        assert_eq!(
            backend.focus_window(target),
            Err(BackendError::UnknownWindow(target))
        );
        let requests = server.finish();

        assert_action_request_sequence(&requests, FOCUS_REQUEST);
    }

    #[test]
    fn non_ok_action_responses_fail_without_postcondition_snapshot() {
        for response in [
            b"ok\n".to_vec(),
            Vec::new(),
            b"okay".to_vec(),
            b" ".to_vec(),
            vec![0xff, 0xfe],
            b"error: return hl.dispatch(...):1: hl.focus: window not found".to_vec(),
        ] {
            let (server, mut backend, target) =
                backend_with_target(vec![MockExchange::new(FOCUS_REQUEST, response)]);

            assert_eq!(
                backend.focus_window(target),
                Err(BackendError::ActionFailed)
            );
            let requests = server.finish();

            assert_eq!(requests.len(), 4);
            assert_eq!(requests[3], FOCUS_REQUEST);
        }
    }

    #[test]
    fn never_known_window_fails_before_opening_a_connection() {
        let server = MockServer::start(Vec::new());
        let transport = HyprlandTransport::with_socket_path(server.path().to_owned());
        let mut backend = HyprlandBackend::new(transport);
        let unknown = WindowId(u64::MAX);

        assert_eq!(
            backend.ensure_floating(unknown),
            Err(BackendError::UnknownWindow(unknown))
        );
        assert!(server.finish().is_empty());
    }

    #[test]
    fn action_connect_failure_is_compositor_unavailable() {
        let server = MockServer::start(action_snapshot_responses(Some(false), true));
        let transport = HyprlandTransport::with_socket_path(server.path().to_owned());
        let mut backend = HyprlandBackend::new(transport);
        let target = backend
            .list_windows()
            .expect("initial target snapshot should normalize")[0]
            .id();
        let requests = server.finish();

        assert_eq!(requests.len(), 3);
        assert_eq!(
            backend.ensure_tiled(target),
            Err(BackendError::CompositorUnavailable)
        );
    }
}
