//! Stateful normalization from Hyprland wire responses to CLEA observations.

use std::collections::{HashMap, HashSet};
use std::fmt;

use super::super::{BackendError, WindowId, WindowObservation, WorkspaceId, WorkspaceObservation};
use super::wire;
use crate::core::WindowPlacement;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct NativeWorkspaceId(i64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct NativeStableId(u64);

impl NativeStableId {
    fn parse(value: &str) -> Result<Self, BackendError> {
        u64::from_str_radix(value, 16)
            .map(Self)
            .map_err(|_| BackendError::InconsistentObservedState)
    }
}

impl fmt::LowerHex for NativeStableId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::LowerHex::fmt(&self.0, formatter)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct HyprlandAddress(String);

#[derive(Clone)]
struct HyprlandWindowIdentity {
    id: WindowId,
    address: HyprlandAddress,
}

struct ClientState {
    stable_id: NativeStableId,
    address: HyprlandAddress,
    workspace_id: NativeWorkspaceId,
    placement: WindowPlacement,
    fullscreen: bool,
}

pub(super) struct HyprlandSnapshot {
    pub(super) workspaces: Vec<WorkspaceObservation>,
    pub(super) windows: Vec<WindowObservation>,
}

#[derive(Clone, Default)]
pub(super) struct HyprlandSnapshotSource {
    next_workspace_id: u64,
    next_window_id: u64,
    workspace_ids: HashMap<NativeWorkspaceId, WorkspaceId>,
    windows_by_stable_id: HashMap<NativeStableId, HyprlandWindowIdentity>,
    window_ids_by_address: HashMap<HyprlandAddress, WindowId>,
}

impl HyprlandSnapshotSource {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn normalize(
        &mut self,
        workspaces_json: &str,
        clients_json: &str,
        active_window_json: &str,
    ) -> Result<HyprlandSnapshot, BackendError> {
        let workspaces = prepare_workspaces(wire::parse_workspaces(workspaces_json)?)?;
        let clients = prepare_clients(wire::parse_clients(clients_json)?)?;
        let active_window = wire::parse_active_window(active_window_json)?;
        let focused_index = correlate_active_window(&clients, active_window)?;

        // Normalize transactionally so an invalid snapshot cannot partially
        // update the identities retained by this source.
        let mut staged = self.clone();
        let snapshot = staged.build_snapshot(workspaces, clients, focused_index)?;
        *self = staged;
        Ok(snapshot)
    }

    pub(super) fn native_stable_id(&self, window_id: WindowId) -> Option<NativeStableId> {
        self.windows_by_stable_id
            .iter()
            .find_map(|(stable_id, identity)| (identity.id == window_id).then_some(*stable_id))
    }

    fn build_snapshot(
        &mut self,
        workspaces: Vec<wire::Workspace>,
        clients: Vec<ClientState>,
        focused_index: usize,
    ) -> Result<HyprlandSnapshot, BackendError> {
        let workspaces = workspaces
            .into_iter()
            .map(|workspace| {
                self.workspace_id(NativeWorkspaceId(workspace.id))
                    .map(|id| WorkspaceObservation::new(id, true))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut windows = Vec::with_capacity(clients.len());
        for (index, client) in clients.into_iter().enumerate() {
            let workspace_id = self.workspace_id(client.workspace_id)?;
            let window_id = self.window_id(client.stable_id, client.address)?;
            windows.push(WindowObservation::new(
                window_id,
                workspace_id,
                client.placement,
                client.fullscreen,
                index == focused_index,
            ));
        }

        Ok(HyprlandSnapshot {
            workspaces,
            windows,
        })
    }

    fn workspace_id(&mut self, native_id: NativeWorkspaceId) -> Result<WorkspaceId, BackendError> {
        if let Some(id) = self.workspace_ids.get(&native_id) {
            return Ok(*id);
        }

        let id = WorkspaceId(self.next_workspace_id);
        self.next_workspace_id = self
            .next_workspace_id
            .checked_add(1)
            .ok_or(BackendError::InconsistentObservedState)?;
        self.workspace_ids.insert(native_id, id);
        Ok(id)
    }

    fn window_id(
        &mut self,
        stable_id: NativeStableId,
        address: HyprlandAddress,
    ) -> Result<WindowId, BackendError> {
        if let Some(identity) = self.windows_by_stable_id.get(&stable_id) {
            let id = identity.id;
            let previous_address = identity.address.clone();

            if previous_address != address {
                if self.window_ids_by_address.get(&previous_address) == Some(&id) {
                    self.window_ids_by_address.remove(&previous_address);
                }
                let identity = self
                    .windows_by_stable_id
                    .get_mut(&stable_id)
                    .ok_or(BackendError::InconsistentObservedState)?;
                identity.address = address.clone();
            }

            self.window_ids_by_address.insert(address, id);
            return Ok(id);
        }

        let id = WindowId(self.next_window_id);
        self.next_window_id = self
            .next_window_id
            .checked_add(1)
            .ok_or(BackendError::InconsistentObservedState)?;
        self.windows_by_stable_id.insert(
            stable_id,
            HyprlandWindowIdentity {
                id,
                address: address.clone(),
            },
        );
        self.window_ids_by_address.insert(address, id);
        Ok(id)
    }
}

fn prepare_workspaces(
    workspaces: Vec<wire::Workspace>,
) -> Result<Vec<wire::Workspace>, BackendError> {
    let mut native_ids = HashSet::with_capacity(workspaces.len());

    for workspace in &workspaces {
        if !native_ids.insert(NativeWorkspaceId(workspace.id)) {
            return Err(BackendError::InconsistentObservedState);
        }
    }

    Ok(workspaces)
}

fn prepare_clients(clients: Vec<wire::Client>) -> Result<Vec<ClientState>, BackendError> {
    let mut stable_ids = HashMap::with_capacity(clients.len());
    let mut addresses = HashMap::with_capacity(clients.len());
    let mut prepared = Vec::with_capacity(clients.len());

    for client in clients {
        let stable_id = NativeStableId::parse(&client.stable_id)?;
        let address = HyprlandAddress(client.address);

        if stable_ids.insert(stable_id, ()).is_some()
            || addresses.insert(address.clone(), ()).is_some()
        {
            return Err(BackendError::InconsistentObservedState);
        }

        let placement = if client.floating {
            WindowPlacement::Floating
        } else {
            WindowPlacement::Tiled
        };

        let fullscreen = match client.fullscreen {
            0 | 1 => false,
            2 | 3 => true,
            _ => return Err(BackendError::InconsistentObservedState),
        };

        prepared.push(ClientState {
            stable_id,
            address,
            workspace_id: NativeWorkspaceId(client.workspace.id),
            placement,
            fullscreen,
        });
    }

    Ok(prepared)
}

fn correlate_active_window(
    clients: &[ClientState],
    active_window: wire::ActiveWindow,
) -> Result<usize, BackendError> {
    let active_stable_id = NativeStableId::parse(&active_window.stable_id)?;
    let stable_match = clients
        .iter()
        .position(|client| client.stable_id == active_stable_id);
    let address_match = clients
        .iter()
        .position(|client| client.address.0 == active_window.address);

    match (stable_match, address_match) {
        (Some(stable), Some(address)) if stable != address => {
            Err(BackendError::InconsistentObservedState)
        }
        (Some(stable), _) => Ok(stable),
        (None, Some(address)) => Ok(address),
        // This provisional classification does not define retries or recovery
        // for a window that disappears between synchronous Hyprland requests.
        (None, None) => Err(BackendError::ObservedStateUnavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::{HyprlandAddress, HyprlandSnapshotSource, NativeStableId};
    use crate::backend::{BackendError, WindowId};
    use crate::core::WindowPlacement;

    const WORKSPACES_FIXTURE: &str =
        include_str!("../../../tests/fixtures/hyprland/workspaces.json");
    const CLIENTS_FIXTURE: &str = include_str!("../../../tests/fixtures/hyprland/clients.json");
    const ACTIVE_WINDOW_FIXTURE: &str =
        include_str!("../../../tests/fixtures/hyprland/activewindow.json");

    fn clients_json(entries: &[(&str, &str, i64, bool, i64)]) -> String {
        let entries = entries
            .iter()
            .map(|(stable_id, address, workspace_id, floating, fullscreen)| {
                format!(
                    r#"{{"stableId":"{stable_id}","address":"{address}","workspace":{{"id":{workspace_id}}},"floating":{floating},"fullscreen":{fullscreen}}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("[{entries}]")
    }

    fn active_window_json(stable_id: &str, address: &str) -> String {
        format!(r#"{{"stableId":"{stable_id}","address":"{address}"}}"#)
    }

    fn one_client_snapshot(
        source: &mut HyprlandSnapshotSource,
        stable_id: &str,
        address: &str,
        workspace_id: i64,
        floating: bool,
        fullscreen: i64,
    ) -> Result<super::HyprlandSnapshot, BackendError> {
        source.normalize(
            &format!(r#"[{{"id":{workspace_id}}}]"#),
            &clients_json(&[(stable_id, address, workspace_id, floating, fullscreen)]),
            &active_window_json(stable_id, address),
        )
    }

    #[test]
    fn fixtures_produce_normalized_workspace_and_window_observations() {
        let mut source = HyprlandSnapshotSource::new();

        let snapshot = source
            .normalize(WORKSPACES_FIXTURE, CLIENTS_FIXTURE, ACTIVE_WINDOW_FIXTURE)
            .expect("synthetic fixtures should normalize");

        assert_eq!(snapshot.workspaces.len(), 2);
        assert!(
            snapshot
                .workspaces
                .iter()
                .all(|workspace| workspace.is_present())
        );
        assert_ne!(snapshot.workspaces[0].id(), snapshot.workspaces[1].id());
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.windows[0].placement(), WindowPlacement::Tiled);
        assert_eq!(snapshot.windows[1].placement(), WindowPlacement::Floating);
        assert!(!snapshot.windows[0].is_fullscreen());
        assert!(snapshot.windows[1].is_fullscreen());
        assert!(!snapshot.windows[0].is_focused());
        assert!(snapshot.windows[1].is_focused());
        assert_eq!(
            snapshot.windows[0].workspace_id(),
            snapshot.workspaces[0].id()
        );
        assert_eq!(
            snapshot.windows[1].workspace_id(),
            snapshot.workspaces[1].id()
        );
    }

    #[test]
    fn workspace_ids_are_preserved_across_snapshots() {
        let mut source = HyprlandSnapshotSource::new();
        let first = one_client_snapshot(&mut source, "1a", "0x100", -7, false, 0)
            .expect("first snapshot should normalize");
        let second = one_client_snapshot(&mut source, "1a", "0x100", -7, false, 0)
            .expect("second snapshot should normalize");

        assert_eq!(first.workspaces[0].id(), second.workspaces[0].id());
    }

    #[test]
    fn duplicate_workspace_ids_are_rejected() {
        let mut source = HyprlandSnapshotSource::new();
        let workspaces = r#"[{"id":-7},{"id":-7}]"#;

        let result = source.normalize(workspaces, CLIENTS_FIXTURE, ACTIVE_WINDOW_FIXTURE);

        assert!(matches!(
            result,
            Err(BackendError::InconsistentObservedState)
        ));
        assert!(source.workspace_ids.is_empty());
    }

    #[test]
    fn client_can_allocate_workspace_identity_without_workspace_observation() {
        let mut source = HyprlandSnapshotSource::new();
        let clients = clients_json(&[("1a", "0x100", -9, false, 0)]);

        let snapshot = source
            .normalize("[]", &clients, &active_window_json("1a", "0x100"))
            .expect("client workspace should receive a private identity");

        assert!(snapshot.workspaces.is_empty());
        assert_eq!(snapshot.windows.len(), 1);

        let later = one_client_snapshot(&mut source, "1a", "0x100", -9, false, 0)
            .expect("later workspace observation should normalize");
        assert_eq!(snapshot.windows[0].workspace_id(), later.workspaces[0].id());
    }

    #[test]
    fn floating_values_normalize_to_distinct_placements() {
        let mut source = HyprlandSnapshotSource::new();
        let clients = clients_json(&[("1a", "0x100", 1, false, 0), ("2b", "0x200", 1, true, 0)]);
        let snapshot = source
            .normalize("[{\"id\":1}]", &clients, &active_window_json("1a", "0x100"))
            .expect("placements should normalize");

        assert_eq!(snapshot.windows[0].placement(), WindowPlacement::Tiled);
        assert_eq!(snapshot.windows[1].placement(), WindowPlacement::Floating);
    }

    #[test]
    fn fullscreen_domain_is_normalized_without_boolean_coercion() {
        for (mode, expected) in [(0, false), (1, false), (2, true), (3, true)] {
            let mut source = HyprlandSnapshotSource::new();
            let snapshot = one_client_snapshot(&mut source, "1a", "0x100", 1, false, mode)
                .expect("specified fullscreen mode should normalize");

            assert_eq!(snapshot.windows[0].is_fullscreen(), expected);
        }
    }

    #[test]
    fn fullscreen_outside_specified_domain_is_inconsistent() {
        let mut source = HyprlandSnapshotSource::new();

        let result = one_client_snapshot(&mut source, "1a", "0x100", 1, false, 4);

        assert!(matches!(
            result,
            Err(BackendError::InconsistentObservedState)
        ));
    }

    #[test]
    fn different_stable_ids_receive_different_window_ids() {
        let mut source = HyprlandSnapshotSource::new();
        let clients = clients_json(&[("1a", "0x100", 1, false, 0), ("2b", "0x200", 1, false, 0)]);
        let snapshot = source
            .normalize("[{\"id\":1}]", &clients, &active_window_json("1a", "0x100"))
            .expect("distinct clients should normalize");

        assert_ne!(snapshot.windows[0].id(), snapshot.windows[1].id());
    }

    #[test]
    fn hexadecimal_stable_id_is_numeric_and_preserved_across_text_forms() {
        let mut source = HyprlandSnapshotSource::new();
        let first = one_client_snapshot(&mut source, "1a", "0x100", 1, false, 0)
            .expect("lowercase hexadecimal stable ID should normalize");
        let second = one_client_snapshot(&mut source, "01A", "0x100", 1, false, 0)
            .expect("equivalent hexadecimal stable ID should normalize");

        assert_eq!(first.windows[0].id(), second.windows[0].id());
        assert!(
            source
                .windows_by_stable_id
                .contains_key(&NativeStableId(0x1a))
        );
    }

    #[test]
    fn reverse_lookup_uses_the_existing_stable_id_mapping() {
        let mut source = HyprlandSnapshotSource::new();
        let snapshot = one_client_snapshot(&mut source, "1a", "0x100", 1, false, 0)
            .expect("window identity should normalize");

        assert_eq!(
            source.native_stable_id(snapshot.windows[0].id()),
            Some(NativeStableId(0x1a))
        );
        assert_eq!(source.native_stable_id(WindowId(u64::MAX)), None);
    }

    #[test]
    fn invalid_stable_id_returns_error_without_panicking() {
        let mut source = HyprlandSnapshotSource::new();

        let result = one_client_snapshot(&mut source, "not-hex", "0x100", 1, false, 0);

        assert!(matches!(
            result,
            Err(BackendError::InconsistentObservedState)
        ));
    }

    #[test]
    fn changed_address_preserves_window_id_and_updates_private_mapping() {
        let mut source = HyprlandSnapshotSource::new();
        let first = one_client_snapshot(&mut source, "1a", "0x100", 1, false, 0)
            .expect("first address should normalize");
        let second = one_client_snapshot(&mut source, "1a", "0x101", 1, false, 0)
            .expect("changed address should normalize");

        assert_eq!(first.windows[0].id(), second.windows[0].id());
        assert!(
            !source
                .window_ids_by_address
                .contains_key(&HyprlandAddress("0x100".to_owned()))
        );
        assert_eq!(
            source
                .window_ids_by_address
                .get(&HyprlandAddress("0x101".to_owned())),
            Some(&second.windows[0].id())
        );
    }

    #[test]
    fn active_window_uses_stable_id_as_primary_correlation() {
        let mut source = HyprlandSnapshotSource::new();
        let clients = clients_json(&[("1a", "0x100", 1, false, 0), ("2b", "0x200", 1, false, 0)]);
        let snapshot = source
            .normalize("[{\"id\":1}]", &clients, &active_window_json("2b", "0x999"))
            .expect("stable ID alone should correlate focus");

        assert!(!snapshot.windows[0].is_focused());
        assert!(snapshot.windows[1].is_focused());
    }

    #[test]
    fn active_window_falls_back_to_address() {
        let mut source = HyprlandSnapshotSource::new();
        let clients = clients_json(&[("1a", "0x100", 1, false, 0), ("2b", "0x200", 1, false, 0)]);
        let snapshot = source
            .normalize("[{\"id\":1}]", &clients, &active_window_json("ff", "0x200"))
            .expect("address should provide secondary focus correlation");

        assert!(!snapshot.windows[0].is_focused());
        assert!(snapshot.windows[1].is_focused());
    }

    #[test]
    fn contradictory_active_window_identities_are_rejected() {
        let mut source = HyprlandSnapshotSource::new();
        let clients = clients_json(&[("1a", "0x100", 1, false, 0), ("2b", "0x200", 1, false, 0)]);

        let result = source.normalize("[{\"id\":1}]", &clients, &active_window_json("1a", "0x200"));

        assert!(matches!(
            result,
            Err(BackendError::InconsistentObservedState)
        ));
    }

    #[test]
    fn duplicate_numeric_stable_ids_are_rejected() {
        let mut source = HyprlandSnapshotSource::new();
        let clients = clients_json(&[("1a", "0x100", 1, false, 0), ("01A", "0x200", 1, false, 0)]);

        let result = source.normalize("[{\"id\":1}]", &clients, &active_window_json("1a", "0x100"));

        assert!(matches!(
            result,
            Err(BackendError::InconsistentObservedState)
        ));
    }

    #[test]
    fn duplicate_addresses_are_rejected() {
        let mut source = HyprlandSnapshotSource::new();
        let clients = clients_json(&[("1a", "0x100", 1, false, 0), ("2b", "0x100", 1, false, 0)]);

        let result = source.normalize("[{\"id\":1}]", &clients, &active_window_json("1a", "0x100"));

        assert!(matches!(
            result,
            Err(BackendError::InconsistentObservedState)
        ));
    }

    #[test]
    fn uncorrelated_active_window_is_provisionally_unavailable() {
        let mut source = HyprlandSnapshotSource::new();
        let clients = clients_json(&[("1a", "0x100", 1, false, 0)]);

        let result = source.normalize("[{\"id\":1}]", &clients, &active_window_json("2b", "0x200"));

        assert!(matches!(
            result,
            Err(BackendError::ObservedStateUnavailable)
        ));
    }

    #[test]
    fn syntactically_invalid_json_is_unavailable() {
        let mut source = HyprlandSnapshotSource::new();

        let result = source.normalize("[", CLIENTS_FIXTURE, ACTIVE_WINDOW_FIXTURE);

        assert!(matches!(
            result,
            Err(BackendError::ObservedStateUnavailable)
        ));
    }

    #[test]
    fn missing_required_active_window_field_is_inconsistent() {
        let mut source = HyprlandSnapshotSource::new();

        let result = source.normalize(WORKSPACES_FIXTURE, CLIENTS_FIXTURE, "{\"address\":\"0x2\"}");

        assert!(matches!(
            result,
            Err(BackendError::InconsistentObservedState)
        ));
    }

    #[test]
    fn normalized_api_does_not_expose_hyprland_identity_values() {
        let mut source = HyprlandSnapshotSource::new();
        let snapshot = source
            .normalize(WORKSPACES_FIXTURE, CLIENTS_FIXTURE, ACTIVE_WINDOW_FIXTURE)
            .expect("synthetic fixtures should normalize");
        let debug = format!("{:?}", snapshot.windows);

        assert!(!debug.contains("0xabc001"));
        assert!(!debug.contains("0xabc002"));
        assert!(!debug.contains("stableId"));
    }
}
