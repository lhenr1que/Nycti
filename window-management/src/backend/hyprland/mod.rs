//! Read-only Hyprland transport, response parsing, and snapshot normalization.

mod snapshot;
mod transport;
mod wire;

use super::BackendError;
use snapshot::{HyprlandSnapshot, HyprlandSnapshotSource};
use transport::HyprlandTransport;

struct HyprlandReadOnlySource {
    transport: HyprlandTransport,
    snapshots: HyprlandSnapshotSource,
}

impl HyprlandReadOnlySource {
    fn from_env() -> Result<Self, BackendError> {
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
}

#[cfg(test)]
mod tests {
    use super::transport::test_support::{MockExchange, MockServer};
    use super::{HyprlandReadOnlySource, HyprlandTransport};
    use crate::backend::BackendError;
    use crate::core::WindowPlacement;

    const WORKSPACES_FIXTURE: &str =
        include_str!("../../../tests/fixtures/hyprland/workspaces.json");
    const CLIENTS_FIXTURE: &str = include_str!("../../../tests/fixtures/hyprland/clients.json");
    const ACTIVE_WINDOW_FIXTURE: &str =
        include_str!("../../../tests/fixtures/hyprland/activewindow.json");

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

    #[test]
    fn environment_constructor_requires_no_environment_mutation() {
        match HyprlandReadOnlySource::from_env() {
            Ok(_) | Err(BackendError::CompositorUnavailable) => {}
            Err(error) => panic!("unexpected environment resolution error: {error:?}"),
        }
    }

    #[test]
    fn read_only_source_requests_and_normalizes_a_snapshot() {
        let server = MockServer::start(fixture_responses(1));
        let transport = HyprlandTransport::with_socket_path(server.path().to_owned());
        let mut source = HyprlandReadOnlySource::new(transport);

        let snapshot = source
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
    fn read_only_source_preserves_ids_across_successive_snapshots() {
        let server = MockServer::start(fixture_responses(2));
        let transport = HyprlandTransport::with_socket_path(server.path().to_owned());
        let mut source = HyprlandReadOnlySource::new(transport);

        let first = source.snapshot().expect("first snapshot should normalize");
        let second = source.snapshot().expect("second snapshot should normalize");
        let requests = server.finish();

        assert_eq!(requests.len(), 6);
        assert_eq!(first.workspaces[0].id(), second.workspaces[0].id());
        assert_eq!(first.workspaces[1].id(), second.workspaces[1].id());
        assert_eq!(first.windows[0].id(), second.windows[0].id());
        assert_eq!(first.windows[1].id(), second.windows[1].id());
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
        let mut subject = HyprlandReadOnlySource::new(subject_transport);

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
        let mut control = HyprlandReadOnlySource::new(control_transport);

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
}
